# check-all.ps1 — 本地 CI 一键检查:后端 fmt/clippy/test + 前端 typecheck/test/build
# 用法: npm run check  |  或 powershell -NoProfile -ExecutionPolicy Bypass -File tools/check-all.ps1
# 参数: -SkipRust 跳过后端; -SkipWeb 跳过前端; -Quick 只跑 test 不跑 build;
#       -StrictTypecheck 历史保留参数(2026-09-08 起 typecheck 已是硬门禁,此开关无差异)
#       -StrictAudit 把 cargo audit 从警告档切为硬门禁(默认仅警告不拦截)
param(
    [switch]$SkipRust,
    [switch]$SkipWeb,
    [switch]$Quick,
    [switch]$StrictTypecheck,
    [switch]$StrictAudit
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
            # -j 2:本机并行链接曾撞 LNK1318/os error 1455(页面文件不足),限并发换稳定
            Invoke-Stage 'cargo test --workspace'   { cargo test --workspace -j 2 }
            # cargo audit:依赖漏洞扫描(RustSec advisory DB,需联网拉取)。
            # 与 vue-tsc 同策略:默认警告档——发现漏洞/警告只打 Yellow WARN 不拦截,
            # 避免历史漏洞阻塞日常开发;加 -StrictAudit 才走 Invoke-Stage 硬拦截(exit 非 0 即 FAIL)。
            # 未安装 cargo-audit 时打印提示并跳过(安装:cargo install cargo-audit --locked)。
            if (Get-Command cargo-audit -ErrorAction SilentlyContinue) {
                if ($StrictAudit) {
                    Invoke-Stage 'cargo audit'      { cargo audit }
                } else {
                    Write-Host "`n===== cargo audit(警告档;-StrictAudit 可切硬门禁)=====" -ForegroundColor Cyan
                    cargo audit
                    if ($LASTEXITCODE -ne 0) { Write-Host '[WARN] cargo audit 发现漏洞或警告(不拦截;-StrictAudit 可切硬门禁)' -ForegroundColor Yellow }
                    else { Write-Host '[ OK ] cargo audit 无已知漏洞' -ForegroundColor Green }
                }
            } else {
                Write-Host "`n===== cargo audit:未安装 cargo-audit,跳过(安装:cargo install cargo-audit --locked)=====" -ForegroundColor Yellow
            }
        } finally { Pop-Location }
    }
}

if (-not $SkipWeb) {
    Push-Location $root
    try {
        # 契约快照:手写 TS 类型与 Rust 后端字段集合比对(漂移即 FAIL,纯 Node 零依赖)
        Invoke-Stage 'contract: check-contract' { node tools/check-contract.mjs }
        # 架构护栏:前端 store 循环依赖 + 组件直改 state + 后端分层规则 C/D/E/G
        # (全部规则已切硬门禁:违规即 exit 1;C/D/E 已于 2026-09-13 清零,G 为 ratchet)
        Invoke-Stage 'arch: check-arch'         { node tools/check-arch.mjs }
        # 测试数自动统计:与 MAINTENANCE.md 记录比对,文档漂移即 WARN(不拦截)
        Write-Host "`n===== 测试数统计(文档漂移检查,警告档)=====" -ForegroundColor Cyan
        node tools/count-tests.mjs
        node tools/count-tests.mjs --check
        if ($LASTEXITCODE -ne 0) { Write-Host '[WARN] 测试数字与 MAINTENANCE.md 不一致(不拦截;请更新文档)' -ForegroundColor Yellow }
        # 前端类型逃逸 ratchet:as never / as unknown as / 非空断言 / any 只降不升
        # (纯 Node 零依赖,与 check-arch/check-contract 同风格;基线见脚本内 BASELINE)
        Invoke-Stage 'web: type-ratchet'        { node tools/check-frontend-lint.mjs }
        # 2026-09-08 附录 D 168 个存量错误已清偿归零,typecheck 恢复硬门禁;
        # -StrictTypecheck 参数保留兼容(已无分支差异)
        Invoke-Stage 'web: vue-tsc --noEmit'    { npm run typecheck -w web }
        Invoke-Stage 'web: vitest run'          { npm test -w web }
        if (-not $Quick) {
            Invoke-Stage 'web: vite build'      { npm run build -w web }
        }
    } finally { Pop-Location }
}

Write-Host "`n===== 全部通过 =====" -ForegroundColor Green
$results | Format-Table -AutoSize
exit 0
