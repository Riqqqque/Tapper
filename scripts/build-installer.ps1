param(
    [string]$Configuration = "Release",
    [string]$Runtime = "win-x64"
)

$ErrorActionPreference = "Stop"

$repoRoot = Split-Path -Parent $PSScriptRoot
$projectPath = Join-Path $repoRoot "Tapper.csproj"
$readmePath = Join-Path $repoRoot "README.md"
$installerScriptPath = Join-Path $repoRoot "installer\\installer.iss"
$publishDir = Join-Path $repoRoot "installer-build\\publish"
$outputDir = Join-Path $repoRoot "installer-dist"

if (-not (Test-Path $projectPath)) {
    throw "Project file not found at $projectPath."
}

[xml]$projectXml = Get-Content $projectPath
$version = $projectXml.Project.PropertyGroup.Version | Select-Object -First 1
if ([string]::IsNullOrWhiteSpace($version)) {
    throw "Version is missing from Tapper.csproj."
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

New-Item -ItemType Directory -Path $publishDir -Force | Out-Null
New-Item -ItemType Directory -Path $outputDir -Force | Out-Null

dotnet publish $projectPath `
    -c $Configuration `
    -r $Runtime `
    --source https://api.nuget.org/v3/index.json `
    --self-contained true `
    -o $publishDir `
    -p:PublishSingleFile=true `
    -p:PublishTrimmed=false `
    -p:DebugType=None `
    -p:DebugSymbols=false `
    -p:IncludeNativeLibrariesForSelfExtract=true

Copy-Item $readmePath (Join-Path $publishDir "README.md") -Force

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
