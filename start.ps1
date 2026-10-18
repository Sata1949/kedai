# Kedai 统一启动器(PowerShell)
# 一个脚本管理两个版本:
#   - 测试版(默认):浏览器开发/调试模式,启动 kedai-server.exe + 自动打开浏览器。
#   - 正式版:启动 dist\Kedai-portable\Kedai.exe(便携版桌面应用,数据存 %APPDATA%\com.kedai.app)。
#
# 自动编译:启动前检测产品源码(web/src、server-rs/src、src-tauri/src 等)是否比
# 可执行产物新;过期则自动执行 build.ps1(默认双端同步产出)再启动,避免「改了代码还是旧版」。
# 增量产物:watch.ps1 / build.ps1 -Dev 产出的 debug 后端与 web/dist 会被识别为最新;
# 测试版会选最新的 server 产物,debug 服务自动以 KEDAI_WEB_DIST 指向磁盘 web/dist。
# 同步保障:两端产物各带 <exe>.build.json 构建指纹(tools/Write-BuildStamp.ps1);
# 测试版以磁盘 web/dist 为准,指纹不一致只提示不重建;便携版分支仍会检查并双端重建。
# 用法:
#   .\start.ps1              # 测试版:启动服务并打开浏览器
#   .\start.ps1 -NoBrowser   # 测试版:启动服务但不打开浏览器
#   .\start.ps1 -Build       # 测试版:强制先执行 build.ps1 再启动(忽略过期检测)
#   .\start.ps1 -Portable    # 正式版:启动便携版 Kedai.exe(自动检测重建)
#   .\start.ps1 -NoRebuild   # 跳过自动重建,直接用现有产物启动(快速启动)
#   .\start.ps1 -DryRun      # 只打印检测结果(选中的产物/是否需要构建)后退出
#
# 注意:正式版与测试版共用端口 3001 与同一数据目录,请勿同时运行。

param(
    [switch]$NoBrowser,
    [switch]$Build,
    [switch]$Portable,
    [switch]$NoRebuild,
    # 只打印检测结果后退出(不构建、不启动),用于验证增量产物是否被识别
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent $MyInvocation.MyCommand.Path
. (Join-Path $Root "tools\Write-BuildStamp.ps1")

# ===================== 源码过期检测 =====================
# 分源扫描:web 源码只决定 web/dist 的新鲜度,后端/壳源码只决定 kedai-server / 便携版
# 的新鲜度。旧实现把两者混成一个「最新源码时间」,任何后端改动都会让 web/dist 判过期,
# 增量编译(debug + web/dist)因此永远触发全量 build.ps1。
$WebSourcePaths = @(
    (Join-Path $Root "web\src"),
    (Join-Path $Root "web\public"),
    (Join-Path $Root "web\index.html"),
    (Join-Path $Root "web\vite.config.ts"),
    (Join-Path $Root "web\tsconfig.json"),
    (Join-Path $Root "web\package.json")
)
$RustSourcePaths = @(
    (Join-Path $Root "server-rs\src"),
    (Join-Path $Root "server-rs\build.rs"),
    (Join-Path $Root "server-rs\Cargo.toml"),
    (Join-Path $Root "server-rs\Cargo.lock"),
    (Join-Path $Root "src-tauri\src"),
    (Join-Path $Root "src-tauri\build.rs"),
    (Join-Path $Root "src-tauri\tauri.conf.json"),
    (Join-Path $Root "src-tauri\Cargo.toml"),
    (Join-Path $Root "src-tauri\Cargo.lock"),
    (Join-Path $Root "tools\build-portable.ps1"),
    (Join-Path $Root "build.ps1")
)
$ScanPaths = $WebSourcePaths + $RustSourcePaths

function Get-LatestTimeFromPaths([string[]]$Paths) {
    $latest = Get-Date "2000-01-01"
    foreach ($p in $Paths) {
        if (-not (Test-Path $p)) { continue }
        $item = Get-Item $p
        if ($item.PSIsContainer) {
            $max = Get-ChildItem $p -Recurse -File -ErrorAction SilentlyContinue |
                Measure-Object -Property LastWriteTime -Maximum |
                Select-Object -ExpandProperty Maximum
            if ($null -ne $max -and $max -gt $latest) { $latest = $max }
        } elseif ($item.LastWriteTime -gt $latest) {
            $latest = $item.LastWriteTime
        }
    }
    return $latest
}

function Get-LatestSourceTime { return Get-LatestTimeFromPaths $ScanPaths }
function Get-LatestWebSourceTime { return Get-LatestTimeFromPaths $WebSourcePaths }
function Get-LatestRustSourceTime { return Get-LatestTimeFromPaths $RustSourcePaths }

# 产物是否过期:产物缺失或源码比产物新超过 2 分钟。
# 2 分钟宽容窗口:构建过程/编辑器常会 touch 源文件但内容未变,
# 若按精确比较会每次启动都误判过期而触发 5 分钟全量重编(表现为「改了半天还是旧版」)。
function Test-ArtifactStale([string]$ArtifactPath, [datetime]$SourceTime = (Get-LatestSourceTime)) {
    if (-not (Test-Path $ArtifactPath)) { return $true }
    $artTime = (Get-Item $ArtifactPath).LastWriteTime
    return $SourceTime -gt $artTime.AddMinutes(2)
}

# 两版构建指纹是否不一致:两端 sidecar 齐全且 dist_hash 不同 → 已漂移,必须双端重建。
# sidecar 缺失(旧产物、手删)无法判定,返回 $false,交给上方的 mtime 检测兜底。
function Test-VersionMismatch {
    $testExe = Join-Path $Root "dist\kedai-server.exe"
    $portableExe = Join-Path $Root "dist\Kedai-portable\Kedai.exe"
    if (-not (Test-Path $testExe) -or -not (Test-Path $portableExe)) { return $false }
    $hTest = Read-KedaiDistHash -ExePath $testExe
    $hPort = Read-KedaiDistHash -ExePath $portableExe
    if (-not $hTest -or -not $hPort) { return $false }
    return $hTest -ne $hPort
}

# ===================== 正式版(便携版桌面应用) =====================
if ($Portable) {
    $PortableExe = Join-Path $Root "dist\Kedai-portable\Kedai.exe"
    $mismatch = Test-VersionMismatch
    if ($mismatch -and -not $NoRebuild) {
        Write-Host "[同步] 测试版与便携版构建指纹不一致,自动执行 build.ps1 双端重建 ..." -ForegroundColor Yellow
    }
    if (-not $NoRebuild -and ($mismatch -or (Test-ArtifactStale $PortableExe))) {
        Write-Host "========== Kedai 自动构建(检测到源码更新) ==========" -ForegroundColor Yellow
        Write-Host "[构建] 产物过期或两版漂移,自动执行 build.ps1(双端同步) ..." -ForegroundColor Yellow
        # build.ps1 失败走 throw(不是 exit),异常会直接穿透到本脚本;
        # 必须 try/catch 捕获,否则 $LASTEXITCODE 判断永远执行不到,用户只看到裸红字堆栈。
        try {
            & (Join-Path $Root "build.ps1")
        } catch {
            Write-Host "[错误] 构建失败:$($_.Exception.Message)" -ForegroundColor Red
            Write-Host "       请查看上方错误输出,修复后重跑 .\start.ps1" -ForegroundColor Yellow
            Read-Host "按回车关闭窗口"
            exit 1
        }
        Write-Host "[OK] 构建完成,启动新版本。" -ForegroundColor Green
    } elseif (-not (Test-Path $PortableExe)) {
        Write-Host "[错误] 未找到正式版:$PortableExe" -ForegroundColor Red
        Write-Host "请先执行 .\build.ps1(默认双端同步产出),或使用测试版(.\start.ps1)。" -ForegroundColor Yellow
        exit 1
    }
    Write-Host "========== Kedai 正式版(便携版) ==========" -ForegroundColor Cyan
    Write-Host "[启动] $PortableExe" -ForegroundColor Green
    Start-Process -FilePath $PortableExe -WorkingDirectory (Split-Path $PortableExe)
    Write-Host "[OK] 正式版已启动;数据与日志位于 %APPDATA%\com.kedai.app。" -ForegroundColor Green
    Write-Host "[提示] 正式版与测试版共用端口 3001 与数据目录,运行正式版前请关闭测试版。" -ForegroundColor Yellow
    exit 0
}

# ===================== 测试版(浏览器开发/调试) =====================
# 服务端产物:优先用**最新**的一个,而不是固定顺序。watch.ps1 / build.ps1 -Dev 只产
# server-rs\target\debug,固定首选 dist\ 会永远拿旧 release 并误触发全量构建。
$ServerCandidates = @(
    (Join-Path $Root "dist\kedai-server.exe"),
    (Join-Path $Root "server-rs\target\release\kedai-server.exe"),
    (Join-Path $Root "src-tauri\target\release\kedai-server.exe"),
    (Join-Path $Root "server-rs\target\debug\kedai-server.exe")
)
function Get-NewestServerExe {
    return $ServerCandidates |
        Where-Object { Test-Path $_ } |
        Sort-Object -Property @{ Expression = { (Get-Item $_).LastWriteTime } } -Descending |
        Select-Object -First 1
}
$ServerExe = Get-NewestServerExe
$IsDebugServer = [bool]($ServerExe -and ($ServerExe -like "*\target\debug\*"))
$WebDist = Join-Path $Root "web\dist\index.html"

Write-Host "========== Kedai Browser Dev ==========" -ForegroundColor Cyan

# 健康检查:服务是否已在运行(须在下方构建分支调用前定义)
function Test-ServerRunning {
    try {
        $resp = Invoke-WebRequest -Uri "http://127.0.0.1:3001/api/health" -UseBasicParsing -TimeoutSec 2
        return $resp.StatusCode -eq 200
    } catch { return $false }
}

# 0) 构建触发:强制构建(-Build)/ 产物缺失 / 源码过期(-NoRebuild 跳过全部)
# 测试版统一以磁盘 web/dist 为准(KEDAI_WEB_DIST):server 只看 Rust/壳源码,
# web/dist 只看 web 源码,不再因为「两版指纹不一致」触发全量构建。
$NeedBuild = $Build
if (-not $NeedBuild -and -not $NoRebuild) {
    $serverSourceTime = Get-LatestRustSourceTime
    if (-not $ServerExe -or -not (Test-Path $WebDist)) {
        $NeedBuild = $true
    } elseif (Test-ArtifactStale $ServerExe $serverSourceTime) {
        $NeedBuild = $true
    } elseif ((Get-LatestWebSourceTime) -gt (Get-Item $WebDist).LastWriteTime.AddMinutes(2)) {
        $NeedBuild = $true
    } elseif (Test-VersionMismatch) {
        Write-Host "[提示] 测试版与便携版指纹不一致;测试版以磁盘 web/dist 为准,本次不重建(需要便携版同步请跑 build.ps1)" -ForegroundColor DarkYellow
    }
}
if ($DryRun) {
    Write-Host "[DryRun] server=$ServerExe"
    Write-Host "[DryRun] debug=$IsDebugServer"
    Write-Host "[DryRun] webDist=$WebDist"
    Write-Host "[DryRun] needBuild=$NeedBuild"
    exit 0
}
if ($NeedBuild) {
    Write-Host "[自动构建] 产物缺失或源码更新,执行 build.ps1(双端同步) ..." -ForegroundColor Yellow
    # 同 -Portable 分支:build.ps1 失败是 throw,需捕获而非查 $LASTEXITCODE
    try {
        & (Join-Path $Root "build.ps1")
    } catch {
        throw "构建失败:$($_.Exception.Message)(详见上方错误输出)"
    }
    # 重建后重新探测
    $ServerExe = Get-NewestServerExe
    $IsDebugServer = [bool]($ServerExe -and ($ServerExe -like "*\target\debug\*"))
    # 重建后端口仍被旧实例占用 → 旧服务还在提供旧版本,静默复用会让本次重建白做。
    if (Test-ServerRunning) {
        Write-Host "[警告] 刚完成重建,但 3001 端口仍被旧服务实例占用——浏览器拿到的仍是旧版本。" -ForegroundColor Yellow
        $answer = Read-Host "是否结束占用端口的旧 Kedai 进程(kedai-server.exe / Kedai.exe)并继续启动新版?(Y/N)"
        if ($answer -match '^[Yy]') {
            Get-Process -Name 'kedai-server', 'Kedai', 'kedai-portable' -ErrorAction SilentlyContinue | Stop-Process -Force
            Start-Sleep -Milliseconds 800
            if (Test-ServerRunning) {
                throw "旧进程已结束但 3001 端口仍被占用,请检查是否有其它程序占用该端口"
            }
            Write-Host "[OK] 旧进程已结束,继续启动新版本" -ForegroundColor Green
        } else {
            Write-Host "已取消启动:旧进程不退出,浏览器拿到的仍是旧版本。请手动关闭后重跑 .\start.ps1" -ForegroundColor Yellow
            exit 1
        }
    }
}
if (-not $ServerExe) { throw "未能定位 kedai-server.exe,请检查构建产物" }

if (-not (Test-ServerRunning)) {
    $modeLabel = if ($IsDebugServer) { "debug 增量" } else { "release" }
    Write-Host "[启动] 启动 kedai-server.exe($modeLabel)... $ServerExe" -ForegroundColor Green
    # 测试版统一用磁盘 web/dist:无论 server 是 debug 还是 release,前端增量都立即生效。
    $prevWebDist = $env:KEDAI_WEB_DIST
    $env:KEDAI_WEB_DIST = Join-Path $Root "web\dist"
    try {
        $proc = Start-Process -FilePath $ServerExe -WorkingDirectory $Root -PassThru -WindowStyle Hidden
    } finally {
        if ($null -eq $prevWebDist) {
            Remove-Item Env:\KEDAI_WEB_DIST -ErrorAction SilentlyContinue
        } else {
            $env:KEDAI_WEB_DIST = $prevWebDist
        }
    }

    # 等待就绪(最长 60 秒)
    $deadline = (Get-Date).AddSeconds(60)
    while ((Get-Date) -lt $deadline) {
        if (Test-ServerRunning) { break }
        Start-Sleep -Milliseconds 400
    }
    if (-not (Test-ServerRunning)) {
        Write-Host "[错误] 服务未能就绪,请查看 logs/ 目录。" -ForegroundColor Red
        exit 1
    }
    Write-Host "[OK] 服务已就绪 → http://127.0.0.1:3001" -ForegroundColor Green
} else {
    Write-Host "[OK] 服务已在运行 → http://127.0.0.1:3001" -ForegroundColor Green
}

if (-not $NoBrowser) {
    Start-Process "http://127.0.0.1:3001/"
}

Write-Host "==================================" -ForegroundColor Cyan
Write-Host "按任意键退出(将同时关闭服务)…"
$null = $Host.UI.RawUI.ReadKey("NoEcho,IncludeKeyDown")
if ($proc -and -not $proc.HasExited) {
    Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    Write-Host "[OK] 服务已关闭。"
}
