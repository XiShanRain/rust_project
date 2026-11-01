# 批次 REL（NFR-10）：便携 ZIP 打包。
#
# 产物一律落在 `target\dist\`（不入库）：
#   dist\egui-launcher-<版本>-win-x64\        解包后的目录形状（冒烟测试直接跑这份）
#     egui-launcher.exe
#     locales\zh-CN.json / en.json            可选覆盖层，程序内置也有一份
#   dist\egui-launcher-<版本>-win-x64.zip     分发物
#   dist\egui-launcher-<版本>-win-x64.zip.sha256
#
# 两条刻意的不做（登记在发布说明，不在这里偷偷补）：
#   · 不嵌 exe 文件图标：本仓库现在没有 `.ico`，补它要新增 build-dependency，待裁定；
#   · 不做安装器 / 不做代码签名：NFR-10 把 MSI 与签名都列为可选。
#
# 用法：`powershell -NoProfile -ExecutionPolicy Bypass -File pack-release.ps1`
# 已有现成 release 二进制时加 `-SkipBuild`（跳过 cargo build）。

param(
    [string]$Version = '',
    [string]$Out = '',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path $PSScriptRoot).Path
if ($Out -eq '') { $Out = Join-Path $repo 'target\dist' }

if ($Version -eq '') {
    # 版本号只有一个真值来源：Cargo.toml 的 [package] version。
    # 只在 [package] 到下一个 [section] 之间找——文件里还有别的 `version =` 行
    #（`[target.'cfg(windows)'.dependencies.windows]` 就有一行 `version = "=0.62.2"`），
    # 抓错了一条就把整个产物命名成 `egui-launcher-=0.62.2-win-x64`。
    $toml = Get-Content -LiteralPath (Join-Path $repo 'Cargo.toml') -Encoding UTF8
    $inside = $false
    $line = $null
    foreach ($l in $toml) {
        if ($l -match '^\s*\[') { $inside = ($l -match '^\s*\[package\]'); continue }
        if ($inside -and $l -match '^\s*version\s*=') { $line = $l; break }
    }
    if ($null -eq $line) { throw 'Cargo.toml 的 [package] 段里找不到 version' }
    $Version = ($line -replace '^\s*version\s*=\s*', '' -replace '["\s]', '')
}

$exe = Join-Path $repo 'target\release\egui-launcher.exe'
if (-not $SkipBuild) {
    Push-Location $repo
    try { cargo build --release } finally { Pop-Location }
}
if (-not (Test-Path $exe)) { throw "release 二进制不存在：$exe" }

$stem = "egui-launcher-$Version-win-x64"
$stage = Join-Path $Out $stem
if (Test-Path $stage) { Remove-Item -LiteralPath $stage -Recurse -Force }
New-Item -ItemType Directory -Force -Path (Join-Path $stage 'locales') | Out-Null

Copy-Item -LiteralPath $exe -Destination (Join-Path $stage 'egui-launcher.exe') -Force
foreach ($tag in @('zh-CN', 'en')) {
    $src = Join-Path $repo "locales\$tag.json"
    if (-not (Test-Path $src)) { throw "语言包缺失：$src" }
    Copy-Item -LiteralPath $src -Destination (Join-Path $stage "locales\$tag.json") -Force
}

$zip = Join-Path $Out "$stem.zip"
if (Test-Path $zip) { Remove-Item -LiteralPath $zip -Force }
Compress-Archive -LiteralPath $stage -DestinationPath $zip -CompressionLevel Optimal

$hash = (Get-FileHash -Algorithm SHA256 -LiteralPath $zip).Hash.ToLower()
$hashFile = Join-Path $Out "$stem.zip.sha256"
Set-Content -LiteralPath $hashFile -Value "$hash  $stem.zip" -Encoding ASCII

$exeBytes = (Get-Item -LiteralPath (Join-Path $stage 'egui-launcher.exe')).Length
$zipBytes = (Get-Item -LiteralPath $zip).Length
"{0,-14} {1,12:N0}  {2}" -f 'exe', $exeBytes, (Join-Path $stage 'egui-launcher.exe')
"{0,-14} {1,12:N0}  {2}" -f 'zip', $zipBytes, $zip
"{0,-14} {1,12}  {2}" -f 'sha256', $hash, $hashFile
