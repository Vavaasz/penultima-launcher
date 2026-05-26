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
  $minimapRoot = Join-Path $ClientRoot "minimap"
  if (-not (Test-Path -LiteralPath $minimapRoot)) {
    throw "Client minimap directory not found: $minimapRoot"
  }

  $clientAssetsRoot = Join-Path $ClientRoot "assets"
  $minimapTempRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("penultima-full-minimap-" + [System.Guid]::NewGuid().ToString("N"))
  $minimapTempPayload = Join-Path $minimapTempRoot "minimap"
  $assetsTempPayload = Join-Path $minimapTempRoot "assets"
  New-Item -ItemType Directory -Path $minimapTempPayload -Force | Out-Null
  New-Item -ItemType Directory -Path $assetsTempPayload -Force | Out-Null
  Copy-Item -Path (Join-Path $minimapRoot "*") -Destination $minimapTempPayload -Recurse -Force
  [void](Copy-FullMapAssetFiles -SourceRoot $clientAssetsRoot -DestinationRoot $assetsTempPayload)
  [void](Copy-FullMapAssetFiles -SourceRoot $WorldRoot -DestinationRoot $assetsTempPayload)

  if (Test-Path -LiteralPath $fullMinimapZipPath) {
    Remove-Item -LiteralPath $fullMinimapZipPath -Force
  }

  Compress-Archive -Path (Join-Path $minimapTempRoot "*") -DestinationPath $fullMinimapZipPath -CompressionLevel Optimal -Force
  Remove-Item -LiteralPath $minimapTempRoot -Recurse -Force
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
    zip = "downloads/Penultima-Launcher.zip"
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
  $minimapRoot = Join-Path $ClientRoot "minimap"
  $minimapFileCount = if (Test-Path -LiteralPath $minimapRoot) {
    (Get-ChildItem -LiteralPath $minimapRoot -Recurse -File | Measure-Object).Count
  } else {
    0
  }
  $clientAssetsRoot = Join-Path $ClientRoot "assets"
  $assetPatterns = @("minimap-*", "satellite-*", "map-*", "staticdata-*", "staticmapdata-*")
  $assetNames = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
  foreach ($sourceRoot in @($clientAssetsRoot, $WorldRoot)) {
    if (Test-Path -LiteralPath $sourceRoot) {
      foreach ($pattern in $assetPatterns) {
        Get-ChildItem -LiteralPath $sourceRoot -File -Filter $pattern -ErrorAction SilentlyContinue | ForEach-Object {
          [void]$assetNames.Add($_.Name)
        }
      }
    }
  }
  $assetFileCount = $assetNames.Count

  $fullMinimapMetadata = [ordered]@{
    zip = "downloads/Penultima-Full-Minimap.zip"
    sha256 = (Get-FileHash $fullMinimapZipPath -Algorithm SHA256).Hash
    size = (Get-Item $fullMinimapZipPath).Length
    file_count = $minimapFileCount + $assetFileCount
    minimap_file_count = $minimapFileCount
    asset_file_count = $assetFileCount
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
