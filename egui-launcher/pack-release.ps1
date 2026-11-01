# 批次 REL（NFR-10）：便携 ZIP 打包。
#
# 产物一律落在 `target\dist\`（不入库）：
#   dist\egui-launcher-<版本>-win-x64\        解包后的目录形状（冒烟测试直接跑这份）
#     egui-launcher.exe
#     locales\zh-CN.json / en.json            可选覆盖层，程序内置也有一份
#   dist\egui-launcher-<版本>-win-x64.zip     分发物
#   dist\egui-launcher-<版本>-win-x64.zip.sha256
#   target\dist-build\release\egui-launcher.exe   打包专用构建目录（与日常的 target\release 分开，见下）
#
# 构建面（批次 CRT，NFR-10）：这一步给 cargo 加了 `-C target-feature=+crt-static`，
#   发布物因此**不依赖 VC 运行库**——导入表里的 `VCRUNTIME140.dll` 与 6 条
#   `api-ms-win-crt-*`（共 89 个符号）实测清零，运行期模块表也不再出现 VCRUNTIME140.dll。
#   这个 flag 只活在本脚本这一段里：不进 `.cargo\config.toml`、不进日常 `cargo build` /
#   `cargo test`。理由是它一改就是 400 多个依赖全部重编（实测 8m42s），不该让开发环节
#   每次构建都付这笔钱；也用独立 `--target-dir`，免得把日常那份动态 CRT 的缓存冲掉。
#
# 两条刻意的不做（登记在发布说明，不在这里偷偷补）：
#   · 不嵌 exe 文件图标：本仓库现在没有 `.ico`，补它要新增 build-dependency，待裁定；
#   · 不做安装器 / 不做代码签名：NFR-10 把 MSI 与签名都列为可选。
#
# 用法：`powershell -NoProfile -ExecutionPolicy Bypass -File pack-release.ps1`
# 已有现成的**静态 CRT** release 二进制时加 `-SkipBuild`（跳过 cargo build）；
# 那份二进制必须来自同一套 flag，否则打出来的包又带回 VC 运行库依赖而无人报错。

param(
    [string]$Version = '',
    [string]$Out = '',
    [string]$BuildDir = '',
    [switch]$SkipBuild
)

$ErrorActionPreference = 'Stop'
$repo = (Resolve-Path $PSScriptRoot).Path
if ($Out -eq '') { $Out = Join-Path $repo 'target\dist' }
if ($BuildDir -eq '') { $BuildDir = Join-Path $repo 'target\dist-build' }

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

$exe = Join-Path $BuildDir 'release\egui-launcher.exe'
if (-not $SkipBuild) {
    # RUSTFLAGS 先存后还：调用者自己设的值不该被这一段吃掉（它会影响的是一整次构建的
    # 每一个依赖，静默覆盖比多编两分钟危险得多）。
    $savedRustFlags = $env:RUSTFLAGS
    $env:RUSTFLAGS = (@(@($savedRustFlags) + '-C target-feature=+crt-static') -join ' ').Trim()
    Push-Location $repo
    try {
        cargo build --release --target-dir $BuildDir
        # $ErrorActionPreference 管不到外部命令的退出码：cargo 编译失败时脚本会照样往下走，
        # 然后把上一次留下的旧 exe 打进 zip——发布脚本里这是最坏的一种「成功」。
        if ($LASTEXITCODE -ne 0) { throw "cargo build --release 失败（退出码 $LASTEXITCODE）" }
    } finally {
        Pop-Location
        if ($null -eq $savedRustFlags) { Remove-Item -LiteralPath 'Env:RUSTFLAGS' -ErrorAction SilentlyContinue }
        else { $env:RUSTFLAGS = $savedRustFlags }
    }
}
if (-not (Test-Path $exe)) { throw "静态 CRT 的 release 二进制不存在：$exe（去掉 -SkipBuild 让它现编）" }

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
