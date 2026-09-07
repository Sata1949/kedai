# check-all.ps1 — 本地 CI 一键检查:后端 fmt/clippy/test + 前端 typecheck/test/build
# 用法: npm run check  |  或 powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-all.ps1
# 参数: -SkipRust 跳过后端; -SkipWeb 跳过前端; -Quick 只跑 test 不跑 build;
#       -StrictTypecheck 把 vue-tsc 从警告档切回硬门禁(附录 D 清偿完成后使用)
param(
    [switch]$SkipRust,
    [switch]$SkipWeb,
    [switch]$Quick,
    [switch]$StrictTypecheck
)
$ErrorActionPreference = 'Continue'
$root = Split-Path -Parent $PSScriptRoot
$results = @()

# ---- 环境准备:MSVC vcvars64 + cargo 路径 ----
$vcvars = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\2022\BuildTools\VC\Auxiliary\Build\vcvars64.bat"
if (-not (Test-Path $vcvars)) {
    $vswhere = "${env:ProgramFiles(x86)}\Microsoft Visual Studio\Installer\vswhere.exe"
    if (Test-Path $vswhere) {
        $vsPath = & $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null
        if ($vsPath) { $vcvars = Join-Path $vsPath 'VC\Auxiliary\Build\vcvars64.bat' }
    }
}
$cargoBin = Join-Path $env:USERPROFILE '.cargo\bin'
$hasCargo = [bool](Get-Command cargo -ErrorAction SilentlyContinue) -or (Test-Path (Join-Path $cargoBin 'cargo.exe'))

# 把 vcvars64 设置的环境变量一次性导入当前 PowerShell 进程
# (不再每层 cmd /c 嵌套——PS5.1 原生命令引号传递不可靠,曾导致 clippy 误用 Git 的 link.exe)
function Import-VcVars([string]$VcvarsPath) {
    $setOut = cmd /c "`"$VcvarsPath`" >nul 2>&1 && set"
    foreach ($line in $setOut) {
        $i = $line.IndexOf('=')
        if ($i -gt 0) {
            Set-Item -Path ("Env:\" + $line.Substring(0, $i)) -Value $line.Substring($i + 1)
        }
    }
}

function Invoke-Stage([string]$Name, [scriptblock]$Body) {
    Write-Host "`n===== $Name =====" -ForegroundColor Cyan
    $sw = [System.Diagnostics.Stopwatch]::StartNew()
    & $Body
    $code = $LASTEXITCODE
    $sw.Stop()
    if ($code -ne 0) {
        Write-Host "[FAIL] $Name (exit=$code, 耗时 $([math]::Round($sw.Elapsed.TotalSeconds,1))s)" -ForegroundColor Red
        $script:results += [pscustomobject]@{ Stage = $Name; Result = 'FAIL'; Seconds = [math]::Round($sw.Elapsed.TotalSeconds,1) }
        Write-Host "`n===== 汇总: $Name 失败,后续阶段跳过 =====" -ForegroundColor Red
        $results | Format-Table -AutoSize
        exit 1
    }
    Write-Host "[ OK ] $Name (耗时 $([math]::Round($sw.Elapsed.TotalSeconds,1))s)" -ForegroundColor Green
    $script:results += [pscustomobject]@{ Stage = $Name; Result = 'OK'; Seconds = [math]::Round($sw.Elapsed.TotalSeconds,1) }
}

if (-not $SkipRust) {
    if (-not $hasCargo) { Write-Host '未找到 cargo(也不在 ~\.cargo\bin),跳过后端;请先安装 Rust 工具链' -ForegroundColor Yellow }
    elseif (-not (Test-Path $vcvars)) { Write-Host "未找到 vcvars64.bat,跳过后端;请安装 VS BuildTools(MSVC)" -ForegroundColor Yellow }
    else {
        Import-VcVars $vcvars
        if (-not (Get-Command cargo -ErrorAction SilentlyContinue)) { $env:PATH = "$cargoBin;$env:PATH" }
        Push-Location "$root\server-rs"
        try {
            Invoke-Stage 'cargo fmt --check'        { cargo fmt --check }
            Invoke-Stage 'cargo clippy'             { cargo clippy --all-targets -- -D warnings }
            Invoke-Stage 'cargo test --workspace'   { cargo test --workspace }
        } finally { Pop-Location }
    }
}

if (-not $SkipWeb) {
    Push-Location $root
    try {
        if ($StrictTypecheck) {
            Invoke-Stage 'web: vue-tsc --noEmit'    { npm run typecheck -w web }
        } else {
            Write-Host "`n===== web: vue-tsc --noEmit(仅警告,存量清偿见 docs/优化实施方案-2026-09.md 附录 D)=====" -ForegroundColor Cyan
            npm run typecheck -w web
            if ($LASTEXITCODE -ne 0) { Write-Host '[WARN] vue-tsc 存在存量类型错误(不拦截;-StrictTypecheck 可切硬门禁)' -ForegroundColor Yellow }
            else { Write-Host '[ OK ] vue-tsc 全绿' -ForegroundColor Green }
        }
        Invoke-Stage 'web: vitest run'          { npm test -w web }
        if (-not $Quick) {
            Invoke-Stage 'web: vite build'      { npm run build -w web }
        }
    } finally { Pop-Location }
}

Write-Host "`n===== 全部通过 =====" -ForegroundColor Green
$results | Format-Table -AutoSize
exit 0
