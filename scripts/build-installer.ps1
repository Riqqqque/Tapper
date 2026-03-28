param(
    [ValidateSet("debug", "release")]
    [string]$Configuration = "release",
    [string]$Target = "x86_64-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $repoRoot "Cargo.toml"
$readmePath = Join-Path $repoRoot "README.md"
$settingsPath = Join-Path $repoRoot "tapper.settings.json"
$installerScriptPath = Join-Path $repoRoot "installer\\installer.iss"
$publishDir = Join-Path $repoRoot "installer-build\\publish"
$distDir = Join-Path $repoRoot "dist"
$outputDir = Join-Path $repoRoot "installer-dist"

if (-not (Test-Path $manifestPath)) {
    throw "Cargo.toml was not found at $manifestPath."
}

$manifest = Get-Content $manifestPath -Raw
$versionMatch = [regex]::Match($manifest, '(?m)^\s*version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw "Version is missing from Cargo.toml."
}

$version = $versionMatch.Groups[1].Value

$cargoCandidates = @(
    (Get-Command cargo.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue),
    (Join-Path $env:USERPROFILE ".cargo\\bin\\cargo.exe")
) | Where-Object { $_ -and (Test-Path $_) }

$cargoPath = $cargoCandidates | Select-Object -First 1
if (-not $cargoPath) {
    throw "cargo.exe was not found. Install Rust first."
}

$isccCandidates = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue),
    (Join-Path $env:LOCALAPPDATA "Programs\\Inno Setup 6\\ISCC.exe"),
    "C:\\Program Files (x86)\\Inno Setup 6\\ISCC.exe",
    "C:\\Program Files\\Inno Setup 6\\ISCC.exe"
) | Where-Object { $_ -and (Test-Path $_) }

$isccPath = $isccCandidates | Select-Object -First 1
if (-not $isccPath) {
    throw "ISCC.exe was not found. Install Inno Setup 6 first."
}

if (Test-Path $publishDir) {
    Remove-Item $publishDir -Recurse -Force
}

if (Test-Path $distDir) {
    Remove-Item $distDir -Recurse -Force
}

if (Test-Path $outputDir) {
    Remove-Item $outputDir -Recurse -Force
}

New-Item -ItemType Directory -Path $publishDir -Force | Out-Null
New-Item -ItemType Directory -Path $distDir -Force | Out-Null
New-Item -ItemType Directory -Path $outputDir -Force | Out-Null

& $cargoPath build `
    --manifest-path $manifestPath `
    --target $Target `
    $(if ($Configuration -eq "release") { "--release" }) `
    --locked

$binaryDir = Join-Path $repoRoot "target\\$Target\\$Configuration"
$binaryPath = Join-Path $binaryDir "Tapper.exe"
if (-not (Test-Path $binaryPath)) {
    throw "Build finished without creating $binaryPath."
}

foreach ($destination in @($publishDir, $distDir)) {
    Copy-Item $binaryPath (Join-Path $destination "Tapper.exe") -Force
    Copy-Item $settingsPath (Join-Path $destination "tapper.settings.json") -Force
    Copy-Item $readmePath (Join-Path $destination "README.md") -Force
}

& $isccPath `
    /Qp `
    "/DAppVersion=$version" `
    "/DPublishDir=$publishDir" `
    "/DOutputDir=$outputDir" `
    $installerScriptPath

$installer = Get-ChildItem $outputDir -Filter "TapperSetup-*.exe" |
    Sort-Object LastWriteTime -Descending |
    Select-Object -First 1

if (-not $installer) {
    throw "Installer build finished without creating a setup executable."
}

Write-Host "Installer created at $($installer.FullName)"
Write-Host "Dist folder refreshed at $distDir"
