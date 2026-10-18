# Kedai 增量编译监视器(开发内循环)
#
# 与 build.ps1 的分工:
#   build.ps1        交付构建:门禁 + 前端 + release + 便携版;收尾删除 target 释放磁盘
#   tools\watch.ps1  开发构建:监听源码,变更后只做增量编译;保留 target,不跑门禁
#
# 监听范围:
#   后端  server-rs\src\**、build.rs、Cargo.toml、Cargo.lock
#   前端  web\src\**、web\public\**、index.html、vite.config.*、tsconfig*.json、package.json
#
# 默认行为:
#   后端变更 -> cargo build(debug 增量;产物 server-rs\target\debug\kedai-server.exe)
#   前端变更 -> npm run build -w web(产物 web\dist)
#   注意:后端二进制默认内嵌编译期的 web/dist。开发时若想让前端改动立即生效,
#   用 KEDAI_WEB_DIST=web\dist 启动服务(见 README);若要让内嵌产物也刷新,加 -EmbedWeb。
#
# 用法:
#   .\tools\watch.ps1                  # 监听两端,Ctrl+C 退出
#   .\tools\watch.ps1 -Once            # 只做一次增量编译后退出
#   .\tools\watch.ps1 -RustOnly        # 只监听后端
#   .\tools\watch.ps1 -WebOnly         # 只监听前端
#   .\tools\watch.ps1 -Check           # 后端改用 cargo check(更快,不产出 exe)
#   .\tools\watch.ps1 -Release         # 后端用 release profile(不删 target)
#   .\tools\watch.ps1 -EmbedWeb        # 前端构建成功后顺带重编后端,刷新内嵌 dist
#   .\tools\watch.ps1 -DryRun          # 只打印将要执行的命令(验证监听用)
#
# 退出码:0 正常;-Once 时若最后一次构建失败则为 1。

param(
    [switch]$RustOnly,
    [switch]$WebOnly,
    [switch]$Check,
    [switch]$Release,
    [switch]$EmbedWeb,
    [switch]$Once,
    [switch]$NoInitial,
    [switch]$DryRun,
    [int]$DebounceMs = 800
)

$ErrorActionPreference = "Stop"

if ($RustOnly -and $WebOnly) { throw "-RustOnly 与 -WebOnly 不能同时使用" }
if ($Check -and $Release) { throw "-Check 与 -Release 不能同时使用" }
if ($WebOnly -and ($Check -or $Release)) { throw "-WebOnly 不涉及 cargo,-Check/-Release 无意义" }
if ($DebounceMs -lt 100) { throw "-DebounceMs 至少 100 毫秒" }

$Root = Split-Path -Parent $PSScriptRoot
$CargoWrapper = Join-Path $Root "tools\cargo-vcvars.cmd"
$WriteStamp = Join-Path $Root "tools\Write-BuildStamp.ps1"
if (-not (Test-Path $CargoWrapper)) { throw "缺少 cargo 包装器:$CargoWrapper" }
if (-not (Test-Path $WriteStamp)) { throw "缺少构建脚本依赖:$WriteStamp" }
. $WriteStamp

$WatchWeb = -not $RustOnly
$WatchRust = -not $WebOnly
$script:LastExit = 0
$script:Watchers = @()

# 只在特定文件变化时触发编译;其余(编辑器临时文件、VCS 目录、构建产物)一律忽略。
$RustConfigNames = @("Cargo.toml", "Cargo.lock", "build.rs")
$WebConfigNames = @(
    "index.html", "vite.config.ts", "vite.config.js", "vite.config.mts",
    "tsconfig.json", "tsconfig.node.json", "tsconfig.app.json",
    "package.json", "package-lock.json"
)

function Get-TimeStamp { return (Get-Date).ToString("HH:mm:ss") }

function Write-Log {
    param([string]$Level, [string]$Message, [ConsoleColor]$Color = [ConsoleColor]::Gray)
    Write-Host ("[{0}] [{1}] {2}" -f (Get-TimeStamp), $Level, $Message) -ForegroundColor $Color
}

function Write-BuildResult {
    param([string]$Name, [int]$Code, [double]$Seconds)
    if ($Code -eq 0) {
        Write-Log " OK " ("{0}完成({1:N1}s)" -f $Name, $Seconds) Green
    } else {
        Write-Log "FAIL" ("{0}失败(exit={1});修复后保存文件会自动重试" -f $Name, $Code) Red
    }
}

# 统一经 cmd 合并 stdout/stderr:PowerShell 5.1 会把 native stderr 包成 ErrorRecord,
# 即使编译成功也会出现红字。成败只看退出码。
function Invoke-NativeCommand {
    param([string]$Display, [string]$CommandLine)
    Write-Log "build" $Display Cyan
    if ($DryRun) {
        Write-Host ("           $ " + $CommandLine) -ForegroundColor DarkGray
        return 0
    }
    $ErrorActionPreference = "Continue"
    try {
        # Out-Host:实时显示编译输出,但不让这些字符串混进函数返回值(返回值只留退出码)。
        & $env:ComSpec /d /c ($CommandLine + " 2>&1") | Out-Host
        return $LASTEXITCODE
    } finally {
        $ErrorActionPreference = "Stop"
    }
}

function Invoke-RustBuild {
    $profile = if ($Release) { "release" } else { "debug" }
    if ($Check) {
        $cargoArgs = "cargo check"
        $display = "cargo check(增量检查,不产出 exe)"
    } elseif ($Release) {
        $cargoArgs = "cargo build --release"
        $display = "cargo build --release(增量)"
    } else {
        $cargoArgs = "cargo build"
        $display = "cargo build(debug 增量)"
    }

    # 运行中的 kedai-server.exe 会锁住链接产物;与 build.ps1 同一套改名让位逻辑。
    if (-not $Check -and -not $DryRun) {
        Clear-KedaiLockedServerArtifacts -TargetDir (Join-Path $Root "server-rs\target")
    }

    $commandLine = '"{0}" {1}' -f $CargoWrapper, $cargoArgs
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    $code = Invoke-NativeCommand -Display $display -CommandLine $commandLine
    $sw.Stop()
    Write-BuildResult -Name "后端编译" -Code $code -Seconds $sw.Elapsed.TotalSeconds
    if ($code -eq 0 -and -not $Check) {
        Write-Log " out " ("server-rs\target\{0}\kedai-server.exe" -f $profile) DarkGray
    }
    return $code
}

function Invoke-WebBuild {
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    Push-Location $Root
    try {
        $code = Invoke-NativeCommand -Display "npm run build -w web(vite 增量)" -CommandLine "npm run build -w web"
    } finally {
        Pop-Location
    }
    $sw.Stop()
    Write-BuildResult -Name "前端构建" -Code $code -Seconds $sw.Elapsed.TotalSeconds
    if ($code -eq 0 -and -not $DryRun) {
        if (-not (Test-Path (Join-Path $Root "web\dist\index.html"))) {
            Write-Log "FAIL" "web\dist\index.html 未生成,请检查 vite 输出" Red
            return 1
        }
        Write-Log " out " "web\dist" DarkGray
    }
    return $code
}

function Test-WatchedChange {
    param([string]$Kind, [string]$FullPath)
    if ([string]::IsNullOrEmpty($FullPath)) { return $false }

    foreach ($segment in @("\target\", "\node_modules\", "\.git\", "\.zcode\", "\dist\", "\logs\")) {
        if ($FullPath -like ("*" + $segment + "*")) { return $false }
    }
    $name = [System.IO.Path]::GetFileName($FullPath)
    if ($name -eq "4913" -or $name.StartsWith(".#")) { return $false }
    if ($name -match "\.(tmp|swp|swx|swpx|bak)$" -or $name -match "~$") { return $false }

    if ($Kind -eq "Rust") {
        if ($FullPath -like "*\server-rs\src\*") { return $true }
        return ($RustConfigNames -contains $name)
    }
    if ($FullPath -like "*\web\src\*" -or $FullPath -like "*\web\public\*") { return $true }
    return ($WebConfigNames -contains $name)
}

function Read-Changes {
    param([hashtable]$Pending)
    $found = $false
    $events = @(Get-Event -ErrorAction SilentlyContinue | Where-Object { $_.SourceIdentifier -like "kedaiwatch:*" })
    foreach ($evt in $events) {
        $sid = [string]$evt.SourceIdentifier
        $full = ""
        try { $full = [string]$evt.SourceEventArgs.FullPath } catch { }
        Remove-Event -EventIdentifier $evt.EventIdentifier -ErrorAction SilentlyContinue
        $parts = $sid.Split(":")
        if ($parts.Length -lt 2) { continue }
        # 缓冲区溢出等内部错误不会带 FullPath;保守起见让两端都重编一次。
        if ($evt.SourceEventArgs -is [System.IO.ErrorEventArgs]) {
            Write-Log "WARN" ("文件监听出错:{0};将强制重编两端" -f $evt.SourceEventArgs.GetException().Message) Yellow
            $Pending.Rust = $true
            $Pending.Web = $true
            $found = $true
            continue
        }
        if (-not (Test-WatchedChange -Kind $parts[1] -FullPath $full)) { continue }
        if ($parts[1] -eq "Rust") { $Pending.Rust = $true } else { $Pending.Web = $true }
        $found = $true
    }
    return $found
}

function Add-Watcher {
    param([string]$Path, [bool]$Recurse, [string]$Kind)
    if (-not (Test-Path $Path)) { return }

    $watcher = New-Object System.IO.FileSystemWatcher
    $watcher.Path = $Path
    $watcher.IncludeSubdirectories = $Recurse
    $watcher.NotifyFilter = (
        [System.IO.NotifyFilters]::FileName -bor
        [System.IO.NotifyFilters]::LastWrite -bor
        [System.IO.NotifyFilters]::CreationTime -bor
        [System.IO.NotifyFilters]::Size
    )
    # 64KB 是 FileSystemWatcher 的有效上限;批量保存/构建时减少缓冲区溢出丢事件。
    $watcher.InternalBufferSize = 65536

    foreach ($eventName in @("Changed", "Created", "Deleted", "Renamed", "Error")) {
        $sid = "kedaiwatch:{0}:{1}" -f $Kind, [Guid]::NewGuid().ToString("N")
        Register-ObjectEvent -InputObject $watcher -EventName $eventName -SourceIdentifier $sid | Out-Null
    }
    $watcher.EnableRaisingEvents = $true
    $script:Watchers += $watcher
}

function Remove-Watchers {
    foreach ($watcher in $script:Watchers) {
        try {
            $watcher.EnableRaisingEvents = $false
            $watcher.Dispose()
        } catch { }
    }
    Get-EventSubscriber -ErrorAction SilentlyContinue |
        Where-Object { $_.SourceIdentifier -like "kedaiwatch:*" } |
        ForEach-Object { Unregister-Event -SourceIdentifier $_.SourceIdentifier -ErrorAction SilentlyContinue }
    Get-Event -ErrorAction SilentlyContinue |
        Where-Object { $_.SourceIdentifier -like "kedaiwatch:*" } |
        ForEach-Object { Remove-Event -EventIdentifier $_.EventIdentifier -ErrorAction SilentlyContinue }
}

# ===================== 启动信息 =====================
$watchDesc = if ($WatchRust -and $WatchWeb) { "前后端" } elseif ($WatchRust) { "仅后端" } else { "仅前端" }
$rustDesc = if ($Check) { "cargo check" } elseif ($Release) { "cargo build --release" } else { "cargo build(debug)" }

Write-Host "========== Kedai 增量编译监视 ==========" -ForegroundColor Cyan
Write-Host ("  监视范围: {0}" -f $watchDesc)
if ($WatchRust) { Write-Host ("  后端命令: {0} -> server-rs\target\" -f $rustDesc) }
if ($WatchWeb) { Write-Host "  前端命令: npm run build -w web -> web\dist" }
Write-Host ("  防抖窗口: {0} ms;保留 target,不跑门禁" -f $DebounceMs)
if ($EmbedWeb) { Write-Host "  内嵌刷新: 前端构建成功后重编后端" -ForegroundColor Yellow }
if ($DryRun) { Write-Host "  [DryRun] 只打印命令,不实际编译" -ForegroundColor Yellow }
Write-Host "=========================================" -ForegroundColor Cyan

$pending = @{ Rust = $false; Web = $false }

try {
    # 首次编译:先把两端带到最新状态,后续只处理增量。
    if (-not $NoInitial -or $Once) {
        if ($WatchWeb) {
            $code = Invoke-WebBuild
            if ($code -ne 0) {
                $script:LastExit = 1
            } else {
                $script:LastExit = 0
                if ($EmbedWeb) { $pending.Rust = $true }
            }
        }
        $runRust = $WatchRust -or ($EmbedWeb -and $WatchWeb)
        if ($runRust -and $script:LastExit -eq 0) {
            $code = Invoke-RustBuild
            if ($code -ne 0) { $script:LastExit = 1 } else { $script:LastExit = 0 }
        }
        $pending.Rust = $false
        $pending.Web = $false
    }

    if ($Once) {
        Write-Log "done" "单次增量编译结束(exit=$($script:LastExit))" Gray
        exit $script:LastExit
    }

    if ($WatchRust) {
        Add-Watcher -Path (Join-Path $Root "server-rs\src") -Recurse $true -Kind "Rust"
        Add-Watcher -Path (Join-Path $Root "server-rs") -Recurse $false -Kind "Rust"
    }
    if ($WatchWeb) {
        Add-Watcher -Path (Join-Path $Root "web\src") -Recurse $true -Kind "Web"
        Add-Watcher -Path (Join-Path $Root "web\public") -Recurse $true -Kind "Web"
        Add-Watcher -Path (Join-Path $Root "web") -Recurse $false -Kind "Web"
    }

    Write-Log "watch" "已就绪,等待源码变更(Ctrl+C 退出)" Green
    $lastChange = $null

    while ($true) {
        if (Read-Changes -Pending $pending) { $lastChange = Get-Date }

        $due = ($pending.Rust -or $pending.Web) -and $null -ne $lastChange -and
            ((Get-Date) - $lastChange).TotalMilliseconds -ge $DebounceMs
        if ($due) {
            $doWeb = $pending.Web
            $doRust = $pending.Rust
            $pending.Web = $false
            $pending.Rust = $false
            $lastChange = $null

            if ($doWeb) {
                $code = Invoke-WebBuild
                if ($code -ne 0) {
                    $script:LastExit = 1
                    $doRust = $false
                } else {
                    $script:LastExit = 0
                    if ($EmbedWeb) { $doRust = $true }
                }
            }
            if ($doRust -and ($WatchRust -or $EmbedWeb)) {
                $code = Invoke-RustBuild
                if ($code -ne 0) { $script:LastExit = 1 } else { $script:LastExit = 0 }
            }
            # 构建期间产生的事件已排空,从这里重新计时。
            $lastChange = $null
        }

        Start-Sleep -Milliseconds 150
    }
} finally {
    Remove-Watchers
    Write-Log "exit" "监视已停止" DarkGray
}
