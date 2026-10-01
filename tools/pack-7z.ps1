# 发布归档脚本:把 Windows 便携版打成 dist\Kedai-<版本>-win64-portable.7z。
# 命名与归档内结构对齐历史两份归档(Kedai-0.3.0-A-beta / Kedai-0.4.0-alpha):
# 归档内为 Kedai-portable\ 文件夹 + 其中全部文件;版本取构建指纹 sidecar 的实际构建版本。
#
# 用法:
#   .\tools\pack-7z.ps1                     # 打包现有 dist\Kedai-portable(先做一致性校验)
#   .\tools\pack-7z.ps1 -Build              # 先跑一次 .\build.ps1 全量双端构建,再打包
#   .\tools\pack-7z.ps1 -SevenZip <path>    # 显式指定 7z.exe(默认按 PATH/注册表/常见目录探测)
#
# 压缩器:优先 7z.exe(a -t7z -mx=9);找不到时回退系统自带 bsdtar(--format=7zip,压缩率略低)。
# 出包前一致性校验(任一不过即中止,防止把过期/不同步产物发出去):
#   ① 便携目录三件套齐全(Kedai.exe / README.txt / Kedai.exe.build.json);
#   ② sidecar 版本 == package.json 版本(构建后没再动过版本号);
#   ③ 便携版与测试版(dist\kedai-server.exe)sidecar 的 dist_hash 一致(build.ps1 同款断言)。
param(
    [switch]$Build,
    [string]$SevenZip = ""
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
. (Join-Path $Root "tools\Write-BuildStamp.ps1")

function Find-SevenZipExe([string]$Explicit) {
    if ($Explicit) {
        if (Test-Path $Explicit) { return $Explicit }
        throw "指定的 7z.exe 不存在:$Explicit"
    }
    $cmd = Get-Command 7z.exe -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($cmd) { return $cmd.Source }
    foreach ($regRoot in @("HKLM:\SOFTWARE\7-Zip", "HKCU:\SOFTWARE\7-Zip")) {
        foreach ($key in @("Path64", "Path")) {
            $dir = (Get-ItemProperty -Path $regRoot -Name $key -ErrorAction SilentlyContinue).$key
            if ($dir) {
                $candidate = Join-Path $dir "7z.exe"
                if (Test-Path $candidate) { return $candidate }
            }
        }
    }
    foreach ($dir in @("$env:ProgramFiles\7-Zip", "${env:ProgramFiles(x86)}\7-Zip")) {
        $candidate = Join-Path $dir "7z.exe"
        if (Test-Path $candidate) { return $candidate }
    }
    return $null
}

Write-Host "========== Kedai Pack 7z ==========" -ForegroundColor Cyan

if ($Build) {
    Write-Host "[1/3] 全量双端构建(.\build.ps1,含门禁) ..." -ForegroundColor Green
    & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $Root "build.ps1")
    if ($LASTEXITCODE -ne 0) { throw "构建失败(exit=$LASTEXITCODE),已中止打包" }
} else {
    Write-Host "[1/3] 未指定 -Build,复用现有 dist 产物(跳过构建)" -ForegroundColor Yellow
}

$DistDir = Join-Path $Root "dist"
$PortableDir = Join-Path $DistDir "Kedai-portable"
$PortableExe = Join-Path $PortableDir "Kedai.exe"

Write-Host "[2/3] 出包前一致性校验 ..." -ForegroundColor Green
foreach ($f in @($PortableExe, (Join-Path $PortableDir "README.txt"), "$PortableExe.build.json")) {
    if (-not (Test-Path $f)) {
        throw "缺少 $f;请先跑 .\build.ps1(或本脚本加 -Build)生成便携版"
    }
}

$srcVersion = (Get-Content (Join-Path $Root "package.json") -Raw -Encoding UTF8 | ConvertFrom-Json).version
$stamp = Get-Content "$PortableExe.build.json" -Raw -Encoding UTF8 | ConvertFrom-Json
if (-not $stamp.version) {
    throw "便携版 sidecar 缺 version 字段,无法确定归档版本号;请重跑 .\build.ps1"
}
if ($stamp.version -ne $srcVersion) {
    throw "版本不一致:便携版 sidecar 为 $($stamp.version),源码为 $srcVersion;请重跑 .\build.ps1 后再打包"
}

$hPortable = Read-KedaiDistHash -ExePath $PortableExe
$hServer = Read-KedaiDistHash -ExePath (Join-Path $DistDir "kedai-server.exe")
if (-not $hPortable -or -not $hServer) {
    throw "sidecar 缺 dist_hash(portable=$hPortable, server=$hServer),无法校验双端同步;请重跑 .\build.ps1"
}
if ($hPortable -ne $hServer) {
    throw "双端指纹不一致(portable=$hPortable, server=$hServer),产物已漂移;请重跑 .\build.ps1"
}
Write-Host "[OK] 版本 $srcVersion;双端指纹一致($($hPortable.Substring(0,12))…)" -ForegroundColor DarkGray

# 归档名对齐历史体例:Kedai-<版本>-win64-portable.7z。7z 对已存在归档执行的是「增量更新」,
# 旧文件会残留,故先删同名旧归档再生成——重复出包即完整重生成。
$archive = Join-Path $DistDir "Kedai-$($stamp.version)-win64-portable.7z"
if (Test-Path $archive) {
    Remove-Item $archive -Force
    Write-Host "[提示] 已删除同名旧归档,完整重生成:$(Split-Path $archive -Leaf)" -ForegroundColor Yellow
}

Write-Host "[3/3] 压缩 dist\Kedai-portable ..." -ForegroundColor Green
$sevenZip = Find-SevenZipExe $SevenZip
Push-Location $DistDir
try {
    # 原生工具(7z/bsdtar)的进度与告警常写 stderr,PS 5.1 在 EAP=Stop 下会包成
    # NativeCommandError 误中止;与 build.ps1 同款:临时降为 Continue,成败看退出码。
    $ErrorActionPreference = "Continue"
    if ($sevenZip) {
        Write-Host "[信息] 压缩器:$sevenZip(7z a -t7z -mx=9)" -ForegroundColor DarkGray
        & $sevenZip a -t7z -mx=9 -y $archive "Kedai-portable"
        $code = $LASTEXITCODE
        if ($code -eq 0) {
            & $sevenZip t $archive > $null
            $code = $LASTEXITCODE
        }
        if ($code -ne 0) { throw "7z 打包或完整性自检失败(exit=$code)" }
    } else {
        $bsdtar = Join-Path $env:SystemRoot "System32\tar.exe"
        if (-not (Test-Path $bsdtar)) {
            throw "未找到 7z.exe(PATH/注册表/常见目录均无),且系统缺少 bsdtar($bsdtar);请安装 7-Zip 或用 -SevenZip 指定路径"
        }
        Write-Host "[信息] 未找到 7z.exe,回退系统 bsdtar(--format=7zip,压缩率略低):$bsdtar" -ForegroundColor Yellow
        & $bsdtar -cf $archive --format=7zip "Kedai-portable"
        $code = $LASTEXITCODE
        if ($code -eq 0) {
            $null = & $bsdtar -tf $archive
            $code = $LASTEXITCODE
        }
        if ($code -ne 0) { throw "bsdtar 打包或读回校验失败(exit=$code)" }
    }
} finally {
    $ErrorActionPreference = "Stop"
    Pop-Location
}

$info = Get-Item $archive
$hash = (Get-FileHash -Algorithm SHA256 -Path $archive).Hash.ToLowerInvariant()
Write-Host "==================================" -ForegroundColor Cyan
Write-Host "[OK] 归档:$($info.FullName)" -ForegroundColor Green
Write-Host "     大小:$($info.Length) B($([math]::Round($info.Length / 1MB, 2)) MB)" -ForegroundColor Green
Write-Host "     SHA256:$hash" -ForegroundColor Green
Write-Host "     结构:Kedai-portable 文件夹(Kedai.exe / README.txt / Kedai.exe.build.json)" -ForegroundColor DarkGray
