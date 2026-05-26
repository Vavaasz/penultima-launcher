param(
  [string]$LauncherRoot = "D:\Server\Launcher",
  [string]$ClientRoot = "D:\Server\Cliente-15.23-Prod",
  [string]$WorldRoot = "D:\Server\Penultima-Server\data-otservbr-global\world",
  [string]$WebsiteRoot = "D:\Server\UniServerZ\www",
  [switch]$AllowUnsignedLauncher,
  [string]$CertificateThumbprint = $env:PENULTIMA_SIGN_CERT_THUMBPRINT,
  [string]$CertificatePath = $env:PENULTIMA_SIGN_CERT_PATH,
  [string]$CertificatePassword = $env:PENULTIMA_SIGN_CERT_PASSWORD,
  [switch]$SkipClient,
  [switch]$SkipLauncher,
  [switch]$SkipFullMinimap
)

$ErrorActionPreference = "Stop"

function Get-SafeSignatureStatus {
  param([string]$Path)

  try {
    return (Get-AuthenticodeSignature $Path).Status.ToString()
  } catch {
    return "Unreadable"
  }
}

function Test-HasCertificateConfig {
  param(
    [string]$Thumbprint,
    [string]$Path
  )

  return (-not [string]::IsNullOrWhiteSpace($Thumbprint)) -or (-not [string]::IsNullOrWhiteSpace($Path))
}

function Copy-FullMapAssetFiles {
  param(
    [string]$SourceRoot,
    [string]$DestinationRoot
  )

  if ([string]::IsNullOrWhiteSpace($SourceRoot) -or -not (Test-Path -LiteralPath $SourceRoot)) {
    return 0
  }

  $patterns = @(
    "minimap-*",
    "satellite-*",
    "map-*",
    "staticdata-*",
    "staticmapdata-*"
  )

  $copied = 0
  foreach ($pattern in $patterns) {
    Get-ChildItem -LiteralPath $SourceRoot -File -Filter $pattern -ErrorAction SilentlyContinue | ForEach-Object {
      Copy-Item -LiteralPath $_.FullName -Destination (Join-Path $DestinationRoot $_.Name) -Force
      $copied++
    }
  }

  return $copied
}

function Get-FullMinimapZipCounts {
  param([string]$ZipPath)

  Add-Type -AssemblyName System.IO.Compression.FileSystem
  $archive = [System.IO.Compression.ZipFile]::OpenRead($ZipPath)
  try {
    $minimapCount = 0
    $assetCount = 0
    foreach ($entry in $archive.Entries) {
      if ([string]::IsNullOrWhiteSpace($entry.Name)) {
        continue
      }

      $entryName = $entry.FullName.Replace('\', '/')
      if ($entryName.StartsWith('minimap/', [System.StringComparison]::OrdinalIgnoreCase)) {
        $minimapCount++
      } elseif ($entryName.StartsWith('assets/', [System.StringComparison]::OrdinalIgnoreCase)) {
        $assetCount++
      }
    }

    return [ordered]@{
      Minimap = $minimapCount
      Assets = $assetCount
      Total = $minimapCount + $assetCount
    }
  } finally {
    $archive.Dispose()
  }
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
$fullMinimapZipPath = Join-Path $downloadsRoot "Penultima-Full-Minimap.zip"
$launcherZipPath = Join-Path $downloadsRoot "Penultima-Launcher.zip"
$launcherReleaseDir = Join-Path (Join-Path (Split-Path -Parent $LauncherRoot) "_publish") "penultima-launcher-release"
$publishLauncherScript = Join-Path $LauncherRoot "publish-launcher-release.ps1"
$publishWebsiteClientAssetsScript = Join-Path $ClientRoot "sounds\publish-website-client-assets.ps1"
$fullMinimapBuilderScript = Join-Path $LauncherRoot "tools\build_full_minimap_package.py"
$metadataPath = Join-Path $downloadsRoot "penultima-downloads.json"
$launcherCargoToml = Join-Path $LauncherRoot "Cargo.toml"

New-Item -ItemType Directory -Path $downloadsRoot -Force | Out-Null

$launcherVersionMatch = Select-String -Path $launcherCargoToml -Pattern '^\s*version\s*=\s*"([^"]+)"' |
  Select-Object -First 1
if (-not $launcherVersionMatch) {
  throw "Could not read launcher version from $launcherCargoToml"
}
$launcherVersion = $launcherVersionMatch.Matches[0].Groups[1].Value
$launcherVersionedZipName = "Penultima-Launcher-$launcherVersion.zip"
$launcherVersionedZipPath = Join-Path $downloadsRoot $launcherVersionedZipName

if (-not $SkipClient) {
  if (-not (Test-Path -LiteralPath $publishWebsiteClientAssetsScript)) {
    throw "Canonical client asset publisher not found: $publishWebsiteClientAssetsScript"
  }

  & $publishWebsiteClientAssetsScript `
    -ClientRoot $ClientRoot `
    -WebsiteRoot $WebsiteRoot `
    -Version auto `
    -RebuildMetadata
}

if (-not $SkipLauncher) {
  if ($AllowUnsignedLauncher) {
    Write-Warning "Deploying launcher without Authenticode signing. Configure PENULTIMA_SIGN_CERT_THUMBPRINT on the VPS to publish a signed launcher."
  } elseif (-not (Test-HasCertificateConfig -Thumbprint $CertificateThumbprint -Path $CertificatePath)) {
    throw "Signing is not configured. Set PENULTIMA_SIGN_CERT_THUMBPRINT or PENULTIMA_SIGN_CERT_PATH, or pass -AllowUnsignedLauncher only for local testing."
  }

  $launcherPublishArgs = @{
    ReleaseDir = $launcherReleaseDir
    ZipPath = $launcherZipPath
  }
  if (-not [string]::IsNullOrWhiteSpace($CertificateThumbprint)) {
    $launcherPublishArgs.CertificateThumbprint = $CertificateThumbprint
  }
  if (-not [string]::IsNullOrWhiteSpace($CertificatePath)) {
    $launcherPublishArgs.CertificatePath = $CertificatePath
  }
  if (-not [string]::IsNullOrWhiteSpace($CertificatePassword)) {
    $launcherPublishArgs.CertificatePassword = $CertificatePassword
  }
  if ($AllowUnsignedLauncher) {
    $launcherPublishArgs.AllowUnsigned = $true
  }

  & $publishLauncherScript @launcherPublishArgs
  Copy-Item -LiteralPath $launcherZipPath -Destination $launcherVersionedZipPath -Force
}

if (-not $SkipFullMinimap) {
  if (-not (Test-Path -LiteralPath $fullMinimapBuilderScript)) {
    throw "Full minimap builder not found: $fullMinimapBuilderScript"
  }

  $pythonCommand = Get-Command python -ErrorAction SilentlyContinue
  if (-not $pythonCommand) {
    throw "Python is required to build the full minimap package"
  }

  & $pythonCommand.Source $fullMinimapBuilderScript `
    --client-root $ClientRoot `
    --world-root $WorldRoot `
    --output $fullMinimapZipPath
  if ($LASTEXITCODE -ne 0) {
    throw "Full minimap package build failed with exit code $LASTEXITCODE"
  }
}

$launcherExePath = Join-Path $launcherReleaseDir "penultima-launcher.exe"
$launcherSignatureStatus = $null
$launcherSigned = $null
if (Test-Path $launcherExePath) {
  $launcherSignatureStatus = Get-SafeSignatureStatus -Path $launcherExePath
  $launcherSigned = $launcherSignatureStatus -eq "Valid"
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
    version = $launcherVersion
    zip = "downloads/$launcherVersionedZipName"
    sha256 = (Get-FileHash $launcherZipPath -Algorithm SHA256).Hash
    size = (Get-Item $launcherZipPath).Length
    signed = $launcherSigned
    signature_status = $launcherSignatureStatus
  }

  if (Test-Path $launcherExePath) {
    $launcherMetadata["exe_sha256"] = (Get-FileHash $launcherExePath -Algorithm SHA256).Hash
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

$fullMinimapMetadata = $null
if (Test-Path $fullMinimapZipPath) {
  $fullMinimapCounts = Get-FullMinimapZipCounts -ZipPath $fullMinimapZipPath

  $fullMinimapMetadata = [ordered]@{
    zip = "downloads/Penultima-Full-Minimap.zip"
    sha256 = (Get-FileHash $fullMinimapZipPath -Algorithm SHA256).Hash
    size = (Get-Item $fullMinimapZipPath).Length
    file_count = $fullMinimapCounts.Total
    minimap_file_count = $fullMinimapCounts.Minimap
    asset_file_count = $fullMinimapCounts.Assets
  }
}

$metadata = [ordered]@{
  generated_at_utc = (Get-Date).ToUniversalTime().ToString("o")
  launcher = $launcherMetadata
  portable_client = $portableMetadata
  client_feed = $clientFeedMetadata
  full_minimap = $fullMinimapMetadata
}

$metadataJson = $metadata | ConvertTo-Json -Depth 6
[System.IO.File]::WriteAllText($metadataPath, $metadataJson, [System.Text.UTF8Encoding]::new($false))

Write-Host "Website downloads updated in $downloadsRoot"
