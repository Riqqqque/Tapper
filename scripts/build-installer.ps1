param(
    [ValidateSet("debug", "release")]
    [string]$Configuration = "release",
    [string]$Target = "x86_64-pc-windows-msvc"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$manifestPath = Join-Path $repoRoot "Cargo.toml"
$readmePath = Join-Path $repoRoot "README.md"
$licensePath = Join-Path $repoRoot "LICENSE"
$settingsPath = Join-Path $repoRoot "tapper.settings.json"
$logoPath = Join-Path $repoRoot "assets\logo.png"
$installerScriptPath = Join-Path $repoRoot "installer\installer.iss"
$publishDir = Join-Path $repoRoot "installer-build\publish"
$installerStagingDir = Join-Path $repoRoot "installer-build\output"
$distDir = Join-Path $repoRoot "dist"
$outputDir = Join-Path $repoRoot "installer-dist"

if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "Cargo.toml was not found at $manifestPath."
}

foreach ($requiredPath in @($readmePath, $licensePath, $settingsPath, $logoPath, $installerScriptPath)) {
    if (-not (Test-Path -LiteralPath $requiredPath)) {
        throw "Required build input was not found at $requiredPath."
    }
}

$manifest = Get-Content -LiteralPath $manifestPath -Raw
$versionMatch = [regex]::Match($manifest, '(?m)^\s*version\s*=\s*"([^"]+)"')
if (-not $versionMatch.Success) {
    throw "Version is missing from Cargo.toml."
}

$version = $versionMatch.Groups[1].Value

$cargoCandidates = @(
    (Get-Command cargo.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue),
    (Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe")
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }

$cargoPath = $cargoCandidates | Select-Object -First 1
if (-not $cargoPath) {
    throw "cargo.exe was not found. Install Rust first."
}

$isccCandidates = @(
    (Get-Command ISCC.exe -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Source -ErrorAction SilentlyContinue),
    (Join-Path $env:LOCALAPPDATA "Programs\Inno Setup 6\ISCC.exe"),
    "C:\Program Files (x86)\Inno Setup 6\ISCC.exe",
    "C:\Program Files\Inno Setup 6\ISCC.exe"
) | Where-Object { $_ -and (Test-Path -LiteralPath $_) }

$isccPath = $isccCandidates | Select-Object -First 1
if (-not $isccPath) {
    throw "ISCC.exe was not found. Install Inno Setup 6 first."
}

$buildArguments = @(
    "build",
    "--manifest-path", $manifestPath,
    "--target", $Target,
    "--locked"
)
if ($Configuration -eq "release") {
    $buildArguments += "--release"
}

# Dependency panic messages embed source paths, so keep the local Cargo home
# (and the Windows user name in it) out of the shipped binary.
$cargoHome = if ($env:CARGO_HOME) { $env:CARGO_HOME } else { Join-Path $env:USERPROFILE ".cargo" }
$cargoHome = [System.IO.Path]::GetFullPath($cargoHome).TrimEnd('\')
if ($cargoHome.Contains("'")) {
    throw "Cargo home path cannot contain a single quote: $cargoHome"
}
$buildArguments += @("--config", "target.$Target.rustflags=['--remap-path-prefix=$cargoHome=cargo']")

& $cargoPath @buildArguments
if ($LASTEXITCODE -ne 0) {
    throw "Cargo build failed with exit code $LASTEXITCODE."
}

$binaryDir = Join-Path $repoRoot "target\$Target\$Configuration"
$binaryPath = Join-Path $binaryDir "Tapper.exe"
if (-not (Test-Path -LiteralPath $binaryPath)) {
    throw "Build finished without creating $binaryPath."
}

foreach ($stagingPath in @($publishDir, $installerStagingDir)) {
    if (Test-Path -LiteralPath $stagingPath) {
        Remove-Item -LiteralPath $stagingPath -Recurse -Force
    }
}

New-Item -ItemType Directory -Path $publishDir -Force | Out-Null
New-Item -ItemType Directory -Path $installerStagingDir -Force | Out-Null
$publishAssetsDir = Join-Path $publishDir "assets"
New-Item -ItemType Directory -Path $publishAssetsDir -Force | Out-Null

Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $publishDir "Tapper.exe") -Force
Copy-Item -LiteralPath $settingsPath -Destination (Join-Path $publishDir "tapper.settings.json") -Force
Copy-Item -LiteralPath $readmePath -Destination (Join-Path $publishDir "README.md") -Force
Copy-Item -LiteralPath $licensePath -Destination (Join-Path $publishDir "LICENSE") -Force
Copy-Item -LiteralPath $logoPath -Destination (Join-Path $publishAssetsDir "logo.png") -Force

& $isccPath `
    /Qp `
    "/DAppVersion=$version" `
    "/DPublishDir=$publishDir" `
    "/DOutputDir=$installerStagingDir" `
    $installerScriptPath
if ($LASTEXITCODE -ne 0) {
    throw "Installer compilation failed with exit code $LASTEXITCODE."
}

$stagedInstaller = Join-Path $installerStagingDir "TapperSetup-$version.exe"
if (-not (Test-Path -LiteralPath $stagedInstaller)) {
    throw "Installer build finished without creating $stagedInstaller."
}

foreach ($destination in @($distDir, $outputDir)) {
    if (Test-Path -LiteralPath $destination) {
        Remove-Item -LiteralPath $destination -Recurse -Force
    }
    New-Item -ItemType Directory -Path $destination -Force | Out-Null
}

$distAssetsDir = Join-Path $distDir "assets"
New-Item -ItemType Directory -Path $distAssetsDir -Force | Out-Null
Copy-Item -LiteralPath $binaryPath -Destination (Join-Path $distDir "Tapper.exe") -Force
Copy-Item -LiteralPath $settingsPath -Destination (Join-Path $distDir "tapper.settings.json") -Force
Copy-Item -LiteralPath $readmePath -Destination (Join-Path $distDir "README.md") -Force
Copy-Item -LiteralPath $licensePath -Destination (Join-Path $distDir "LICENSE") -Force
Copy-Item -LiteralPath $logoPath -Destination (Join-Path $distAssetsDir "logo.png") -Force

$installerPath = Join-Path $outputDir "TapperSetup-$version.exe"
Copy-Item -LiteralPath $stagedInstaller -Destination $installerPath -Force

$portableZipPath = Join-Path $outputDir "Tapper-$version-portable.zip"
Compress-Archive -Path (Join-Path $distDir "*") -DestinationPath $portableZipPath -Force

Write-Host "Installer created at $installerPath"
Write-Host "Portable zip created at $portableZipPath"
Write-Host "Dist folder refreshed at $distDir"
Write-Host ""
Write-Host "SHA-256:"
foreach ($artifactPath in @($installerPath, $portableZipPath)) {
    $hash = (Get-FileHash -LiteralPath $artifactPath -Algorithm SHA256).Hash
    Write-Host "- $(Split-Path -Leaf $artifactPath): $hash"
}
