param(
  [string]$LauncherRoot = "D:\Server\Launcher",
  [string]$ClientRoot = "D:\Server\Cliente-15.23-Prod",
  [string]$WebsiteRoot = "D:\Server\UniServerZ\www",
  [switch]$AllowUnsignedLauncher,
  [switch]$SkipClient,
  [switch]$SkipLauncher
)

$ErrorActionPreference = "Stop"

Add-Type -AssemblyName System.IO.Compression
Add-Type -AssemblyName System.IO.Compression.FileSystem

function New-EmptyDirectory([string]$Path) {
  if (Test-Path $Path) {
    Remove-Item -LiteralPath $Path -Recurse -Force
  }
  New-Item -ItemType Directory -Path $Path | Out-Null
}

function New-StagingRoot {
  $path = Join-Path ([System.IO.Path]::GetTempPath()) ("penultima-downloads-" + [guid]::NewGuid().ToString("N"))
  New-Item -ItemType Directory -Path $path | Out-Null
  return $path
}

function New-ZipFromDirectory([string]$SourceDirectory, [string]$ZipPath) {
  if (Test-Path $ZipPath) {
    Remove-Item -LiteralPath $ZipPath -Force
  }

  $zipParent = Split-Path -Parent $ZipPath
  if ($zipParent) {
    New-Item -ItemType Directory -Path $zipParent -Force | Out-Null
  }

  [System.IO.Compression.ZipFile]::CreateFromDirectory(
    $SourceDirectory,
    $ZipPath,
    [System.IO.Compression.CompressionLevel]::Optimal,
    $false
  )
}

function Copy-Tree([string]$SourcePath, [string]$DestinationPath) {
  New-Item -ItemType Directory -Path (Split-Path -Parent $DestinationPath) -Force | Out-Null
  Copy-Item -LiteralPath $SourcePath -Destination $DestinationPath -Recurse -Force
}

function Publish-PortableClient(
  [string]$SourceRoot,
  [string]$PortableZipPath
) {
  $skipDirs = @(".git", "cache", "characterdata", "crashdump", "log", "minimap", "screenshots", "storeimages")
  $stagingRoot = New-StagingRoot
  $portableRoot = Join-Path $stagingRoot "Penultima-Client-Portable"

  try {
    New-Item -ItemType Directory -Path $portableRoot | Out-Null

    Get-ChildItem -LiteralPath $SourceRoot -Force | ForEach-Object {
      if ($skipDirs -contains $_.Name) {
        return
      }

      $targetPath = Join-Path $portableRoot $_.Name
      if ($_.PSIsContainer) {
        Copy-Item -LiteralPath $_.FullName -Destination $targetPath -Recurse -Force
      } else {
        Copy-Item -LiteralPath $_.FullName -Destination $targetPath -Force
      }
    }

    New-ZipFromDirectory -SourceDirectory $portableRoot -ZipPath $PortableZipPath
  }
  finally {
    if (Test-Path $stagingRoot) {
      Remove-Item -LiteralPath $stagingRoot -Recurse -Force -ErrorAction SilentlyContinue
    }
  }
}

function Publish-FeedBootstrapZip(
  [string]$FeedRoot,
  [string]$BootstrapZipPath
) {
  New-ZipFromDirectory -SourceDirectory $FeedRoot -ZipPath $BootstrapZipPath
}

function Should-AllowUnsignedLauncher {
  param([switch]$AllowUnsigned)

  return $AllowUnsigned -or [string]::IsNullOrWhiteSpace($env:PENULTIMA_SIGN_CERT_THUMBPRINT)
}

if (-not (Test-Path $LauncherRoot)) {
  throw "Launcher root not found: $LauncherRoot"
}

if (-not (Test-Path $ClientRoot)) {
  throw "Client root not found: $ClientRoot"
}

if (-not (Test-Path $WebsiteRoot)) {
  throw "Website root not found: $WebsiteRoot"
}

$downloadsRoot = Join-Path $WebsiteRoot "downloads"
$feedRoot = Join-Path $downloadsRoot "client-feed"
$bootstrapZipPath = Join-Path $downloadsRoot "Penultima-Client-Feed.zip"
$portableZipPath = Join-Path $downloadsRoot "Penultima-Client-Portable.zip"
$launcherZipPath = Join-Path $downloadsRoot "Penultima-Launcher.zip"
$launcherReleaseDir = Join-Path (Join-Path (Split-Path -Parent $LauncherRoot) "_publish") "penultima-launcher-release"
$publishClientFeedScript = Join-Path $LauncherRoot "publish-client-feed.ps1"
$publishLauncherScript = Join-Path $LauncherRoot "publish-launcher-release.ps1"
$metadataPath = Join-Path $downloadsRoot "penultima-downloads.json"

New-Item -ItemType Directory -Path $downloadsRoot -Force | Out-Null

if (-not $SkipClient) {
  & $publishClientFeedScript `
    -SourceRoot $ClientRoot `
    -OutputRoot $feedRoot `
    -Version auto

  Publish-FeedBootstrapZip -FeedRoot $feedRoot -BootstrapZipPath $bootstrapZipPath
  Publish-PortableClient -SourceRoot $ClientRoot -PortableZipPath $portableZipPath
}

$allowUnsignedLauncherBuild = Should-AllowUnsignedLauncher -AllowUnsigned:$AllowUnsignedLauncher
if (-not $SkipLauncher) {
  if ($allowUnsignedLauncherBuild) {
    Write-Warning "Deploying launcher without Authenticode signing. Configure PENULTIMA_SIGN_CERT_THUMBPRINT on the VPS to publish a signed launcher."
  }

  $launcherPublishArgs = @{
    ReleaseDir = $launcherReleaseDir
    ZipPath = $launcherZipPath
  }
  if ($allowUnsignedLauncherBuild) {
    $launcherPublishArgs.AllowUnsigned = $true
  }

  & $publishLauncherScript @launcherPublishArgs
}

$feedVersionPath = Join-Path $feedRoot "package.json.version"
$feedVersion = if (Test-Path $feedVersionPath) {
  (Get-Content $feedVersionPath -Raw).Trim()
} else {
  ""
}

$launcherMetadata = $null
if (Test-Path $launcherZipPath) {
  $launcherMetadata = [ordered]@{
    zip = "downloads/Penultima-Launcher.zip"
    sha256 = (Get-FileHash $launcherZipPath -Algorithm SHA256).Hash
    size = (Get-Item $launcherZipPath).Length
    signed = -not $allowUnsignedLauncherBuild
  }
}

$portableMetadata = $null
if (Test-Path $portableZipPath) {
  $portableMetadata = [ordered]@{
    zip = "downloads/Penultima-Client-Portable.zip"
    sha256 = (Get-FileHash $portableZipPath -Algorithm SHA256).Hash
    size = (Get-Item $portableZipPath).Length
  }
}

$clientFeedMetadata = $null
if (Test-Path $bootstrapZipPath) {
  $clientFeedMetadata = [ordered]@{
    version = $feedVersion
    root = "downloads/client-feed"
    bootstrap_zip = "downloads/Penultima-Client-Feed.zip"
    bootstrap_sha256 = (Get-FileHash $bootstrapZipPath -Algorithm SHA256).Hash
    bootstrap_size = (Get-Item $bootstrapZipPath).Length
  }
}

$metadata = [ordered]@{
  generated_at_utc = (Get-Date).ToUniversalTime().ToString("o")
  launcher = $launcherMetadata
  portable_client = $portableMetadata
  client_feed = $clientFeedMetadata
}

$metadata | ConvertTo-Json -Depth 6 | Set-Content -Path $metadataPath -Encoding UTF8

Write-Host "Website downloads updated in $downloadsRoot"
