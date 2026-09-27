# 统一修改全仓库版本号(单一入口,避免 7 处手工同步漏改)。
# 覆盖:根 package.json、web/package.json、server-rs/Cargo.toml、
#       src-tauri/Cargo.toml、launcher/Cargo.toml、src-tauri/tauri.conf.json、
#       package-lock.json(全部自身版本;依赖项版本不动)、
#       web/src/components/settings/AboutSection.vue 的 FALLBACK_VERSION(关于页兜底版本)、
#       MAINTENANCE.md 的版本行与「最后更新」日期。
# Cargo.lock 中包自身版本无需手改,下次 cargo 构建会自动同步。
# 一致性由 build.ps1 开头的 Assert-VersionConsistency 把关(改漏即构建报错)。
#
# 用法:
#   .\tools\bump-version.ps1 0.3.0            # 把全仓库版本号改为 0.3.0
#   .\tools\bump-version.ps1 0.3.0 -DryRun    # 只预览改动,不写文件
#   npm run version:bump -- 0.3.0             # 等效 npm 入口
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Version,
    [switch]$DryRun
)

$ErrorActionPreference = "Stop"
$Root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)

# 版本号须为合法 semver:核心三段 + 可选预发布后缀(`-beta` / `-rc.1` 等)。
# 注意:后缀必须用 `-` 连接——cargo 只接受 `0.3.0-beta`,**不接受** `0.3.0beta`
# (报 unexpected character 'b' after patch version number),故按 semver 校验,
# 并在拒绝时给出可操作的提示。
if ($Version -notmatch '^\d+\.\d+\.\d+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?$') {
    throw "版本号格式非法:「$Version」。支持 x.y.z 或 x.y.z-预发布(如 0.3.0-beta);预发布后缀必须用 - 连接,不能写成 0.3.0beta。"
}

$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
$changed = 0
$skipped = 0

# JSON 类:替换第一处顶层 "version": "x.y.z"
function Update-JsonVersion([string]$RelativePath) {
    $path = Join-Path $Root $RelativePath
    $content = [System.IO.File]::ReadAllText($path)
    $regex = New-Object regex '("version"\s*:\s*")[^"]*(")'
    $m = $regex.Match($content)
    if (-not $m.Success) {
        Write-Host "[警告] $RelativePath 未找到 version 字段,已跳过" -ForegroundColor Yellow
        $script:skipped++
        return
    }
    $old = $m.Groups[0].Value
    $new = $regex.Replace($content, "`${1}$Version`${2}", 1)
    if ($new -eq $content) {
        Write-Host "[跳过] $RelativePath 已是 $Version" -ForegroundColor DarkGray
        $script:skipped++
        return
    }
    Write-Host "[$(if ($DryRun) { '预览' } else { '修改' })] $RelativePath : $old -> `"version`": `"$Version`"" -ForegroundColor Green
    if (-not $DryRun) { [System.IO.File]::WriteAllText($path, $new, $utf8NoBom) }
    $script:changed++
}

# lock 文件:package-lock.json v3 中「自身版本」是值等于产品版本号的 "version" 字段,
# 当前有 3 处:顶层、packages[""](workspace 根)、packages["web"](对应 web/package.json);
# 其余 version 都属依赖包,值不会等于产品版本号(带预发布后缀的如 0.3.0-B-beta 更不会撞值),
# 故**按值精确匹配全部替换**,不按位置计数。
#
# 为何改按值:旧实现写死「前 2 处」(靠「顶层 + packages[""] 恰为前两处」这一位置假设),
# 而包按路径排序后 web 排在 packages 末尾(实测 package-lock.json 行 5853),永远漏改——
# 顶层与 workspace 根改了、web 包没改,lock 与 web/package.json 不一致会被 npm ci 判不同步
# (CI 红)。早期还踩过「不推进搜索偏移导致第二处原地打转」的坑,按值全替换后该问题一并消失。
function Update-LockFileVersion([string]$RelativePath) {
    $path = Join-Path $Root $RelativePath
    $content = [System.IO.File]::ReadAllText($path)
    $first = [regex]::Match($content, '"version"\s*:\s*"([^"]*)"')
    if (-not $first.Success) {
        Write-Host "[警告] $RelativePath 未找到 version 字段,已跳过" -ForegroundColor Yellow
        $script:skipped++
        return
    }
    $oldVersion = $first.Groups[1].Value
    if ($oldVersion -eq $Version) {
        Write-Host "[跳过] $RelativePath 已是 $Version" -ForegroundColor DarkGray
        $script:skipped++
        return
    }
    $regex = New-Object regex ('("version"\s*:\s*")' + [regex]::Escape($oldVersion) + '(")')
    $replaced = $regex.Matches($content).Count
    if ($replaced -lt 3) {
        # 少于 3 处说明 lock 结构变了(workspace 包被删/改名),不猜、不写,交人工核对
        Write-Host "[警告] $RelativePath 只匹配到 $replaced 处自身版本($oldVersion),预期 3 处(顶层 + 两个 workspace 包),已跳过写入" -ForegroundColor Yellow
        $script:skipped++
        return
    }
    $new = $regex.Replace($content, "`${1}$Version`${2}")
    Write-Host "[$(if ($DryRun) { '预览' } else { '修改' })] $RelativePath : 自身版本 $oldVersion x$replaced 处 -> $Version" -ForegroundColor Green
    if (-not $DryRun) { [System.IO.File]::WriteAllText($path, $new, $utf8NoBom) }
    $script:changed++
}

# TOML 类:[package] 段永远位于文件头部,替换行首第一处 version = "x.y.z"
function Update-TomlVersion([string]$RelativePath) {
    $path = Join-Path $Root $RelativePath
    $content = [System.IO.File]::ReadAllText($path)
    $regex = New-Object regex '(?m)^version\s*=\s*"[^"]*"'
    $m = $regex.Match($content)
    if (-not $m.Success) {
        Write-Host "[警告] $RelativePath 未找到 version 行,已跳过" -ForegroundColor Yellow
        $script:skipped++
        return
    }
    $old = $m.Groups[0].Value
    $new = $regex.Replace($content, "version = `"$Version`"", 1)
    if ($new -eq $content) {
        Write-Host "[跳过] $RelativePath 已是 $Version" -ForegroundColor DarkGray
        $script:skipped++
        return
    }
    Write-Host "[$(if ($DryRun) { '预览' } else { '修改' })] $RelativePath : $old -> version = `"$Version`"" -ForegroundColor Green
    if (-not $DryRun) { [System.IO.File]::WriteAllText($path, $new, $utf8NoBom) }
    $script:changed++
}

# 文档:版本行 v 后版本号 + 「最后更新」日期
function Update-MaintenanceDoc([string]$RelativePath) {
    $path = Join-Path $Root $RelativePath
    $content = [System.IO.File]::ReadAllText($path)
    $today = Get-Date -Format "yyyy-MM-dd"
    $new = $content
    $verRegex = New-Object regex '(版本:v)\d+\.\d+\.\d+(?:-[0-9A-Za-z][0-9A-Za-z.-]*)?'
    $m = $verRegex.Match($new)
    if ($m.Success) {
        $new = $verRegex.Replace($new, "`${1}$Version", 1)
        Write-Host "[$(if ($DryRun) { '预览' } else { '修改' })] $RelativePath : $($m.Groups[0].Value) -> 版本:v$Version" -ForegroundColor Green
    } else {
        Write-Host "[警告] $RelativePath 未找到「版本:v...」行,已跳过版本替换" -ForegroundColor Yellow
    }
    $dateRegex = New-Object regex '(最后更新:)\d{4}-\d{2}-\d{2}'
    if ($dateRegex.Match($new).Success) {
        $new = $dateRegex.Replace($new, "`${1}$today", 1)
        Write-Host "[$(if ($DryRun) { '预览' } else { '修改' })] $RelativePath : 最后更新 -> $today" -ForegroundColor Green
    }
    if ($new -ne $content) {
        if (-not $DryRun) { [System.IO.File]::WriteAllText($path, $new, $utf8NoBom) }
        $script:changed++
    } else {
        $script:skipped++
    }
}

# 前端关于页兜底版本常量:后端不可达时关于页展示的版本号,必须与产品版本同步。
# 历史上 0.3.0-A-beta → 0.3.0-B-beta 漏改此常量(FALLBACK_VERSION 停在 A-beta),
# 而该值会经 vite 编进 web/dist 与 exe——漏改即「离线时关于页显示旧号」,肉眼可见。
# 由 build.ps1 的 Assert-VersionConsistency 作为第 8 处声明校验(改漏即构建报错)。
function Update-AboutFallbackVersion([string]$RelativePath) {
    $path = Join-Path $Root $RelativePath
    $content = [System.IO.File]::ReadAllText($path)
    $regex = New-Object regex "(const FALLBACK_VERSION = ')[^']*(')"
    $m = $regex.Match($content)
    if (-not $m.Success) {
        Write-Host "[警告] $RelativePath 未找到 FALLBACK_VERSION 常量,已跳过" -ForegroundColor Yellow
        $script:skipped++
        return
    }
    $old = $m.Groups[0].Value
    $new = $regex.Replace($content, "`${1}$Version`${2}", 1)
    if ($new -eq $content) {
        Write-Host "[跳过] $RelativePath 已是 $Version" -ForegroundColor DarkGray
        $script:skipped++
        return
    }
    Write-Host "[$(if ($DryRun) { '预览' } else { '修改' })] $RelativePath : $old -> FALLBACK_VERSION = '$Version'" -ForegroundColor Green
    if (-not $DryRun) { [System.IO.File]::WriteAllText($path, $new, $utf8NoBom) }
    $script:changed++
}

Write-Host "========== Kedai Bump Version -> $Version $(if ($DryRun) { '(DryRun 预览)' }) ==========" -ForegroundColor Cyan

Update-JsonVersion "package.json"
Update-JsonVersion "web\package.json"
Update-TomlVersion "server-rs\Cargo.toml"
Update-TomlVersion "src-tauri\Cargo.toml"
Update-TomlVersion "launcher\Cargo.toml"
Update-JsonVersion "src-tauri\tauri.conf.json"
Update-AboutFallbackVersion "web\src\components\settings\AboutSection.vue"
Update-LockFileVersion "package-lock.json"
Update-MaintenanceDoc "MAINTENANCE.md"

Write-Host "======================================================" -ForegroundColor Cyan
if ($DryRun) {
    Write-Host "预览完成:将修改 $changed 个文件,跳过 $skipped 个。去掉 -DryRun 执行实际写入。" -ForegroundColor Yellow
} else {
    Write-Host "完成:修改 $changed 个文件,跳过 $skipped 个。" -ForegroundColor Green
    Write-Host "提示:Cargo.lock 中包自身版本会在下次 cargo 构建时自动同步;发版请跑 npm run build:all。" -ForegroundColor Yellow
}
