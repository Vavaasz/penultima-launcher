param(
  [switch]$AllowUnsigned,
  [string]$CertificateThumbprint = $env:PENULTIMA_SIGN_CERT_THUMBPRINT,
  [string]$CertificatePath = $env:PENULTIMA_SIGN_CERT_PATH,
  [string]$CertificatePassword = $env:PENULTIMA_SIGN_CERT_PASSWORD,
  [string]$TimestampUrl = $(if ($env:PENULTIMA_SIGN_TIMESTAMP_URL) { $env:PENULTIMA_SIGN_TIMESTAMP_URL } else { "http://timestamp.digicert.com" }),
  [string]$ReleaseDir = "D:\Server\_publish\penultima-launcher-release",
  [string]$ZipPath = "D:\Server\_publish\Penultima-Launcher.zip"
)

$ErrorActionPreference = "Stop"

function Resolve-SignTool {
  $command = Get-Command signtool.exe -ErrorAction SilentlyContinue
  if ($command) {
    return $command.Source
  }

  $kitsRoot = "C:\Program Files (x86)\Windows Kits\10\bin"
  if (Test-Path $kitsRoot) {
    $candidates = Get-ChildItem $kitsRoot -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
      Sort-Object FullName -Descending
    if ($candidates) {
      return $candidates[0].FullName
    }
  }

  return $null
}

function Get-SafeSignatureStatus {
  param([string]$Path)

  try {
    return (Get-AuthenticodeSignature $Path).Status
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

$cargo = Join-Path $env:USERPROFILE ".cargo\bin\cargo.exe"
if (-not (Test-Path $cargo)) {
  throw "Cargo was not found at $cargo"
}

$root = "D:\Server\Launcher"
$exeSource = Join-Path $root "target\release\penultima-launcher.exe"
$exeTarget = Join-Path $releaseDir "penultima-launcher.exe"

& $cargo build --manifest-path (Join-Path $root "Cargo.toml") --release

if (-not (Test-Path $exeSource)) {
  throw "Launcher executable was not produced at $exeSource"
}

if (-not $AllowUnsigned -and -not (Test-HasCertificateConfig -Thumbprint $CertificateThumbprint -Path $CertificatePath)) {
  throw "Refusing to publish an unsigned launcher. Set PENULTIMA_SIGN_CERT_THUMBPRINT or PENULTIMA_SIGN_CERT_PATH, or pass -AllowUnsigned for a local-only build."
}

if (-not [string]::IsNullOrWhiteSpace($CertificatePath)) {
  if (-not (Test-Path $CertificatePath)) {
    throw "Certificate file not found: $CertificatePath"
  }

  $signTool = Resolve-SignTool
  if (-not $signTool) {
    throw "signtool.exe was not found. Install the Windows SDK or add signtool.exe to PATH."
  }

  $signArgs = @(
    "sign",
    "/f", $CertificatePath,
    "/fd", "SHA256",
    "/td", "SHA256",
    "/tr", $TimestampUrl,
    "/v"
  )
  if (-not [string]::IsNullOrWhiteSpace($CertificatePassword)) {
    $signArgs += @("/p", $CertificatePassword)
  }
  $signArgs += $exeSource

  & $signTool @signArgs
  if ($LASTEXITCODE -ne 0) {
    throw "signtool.exe failed with exit code $LASTEXITCODE"
  }
}
elseif (-not [string]::IsNullOrWhiteSpace($CertificateThumbprint)) {
  $signTool = Resolve-SignTool
  if (-not $signTool) {
    throw "signtool.exe was not found. Install the Windows SDK or add signtool.exe to PATH."
  }

  & $signTool sign /sha1 $CertificateThumbprint /fd SHA256 /td SHA256 /tr $TimestampUrl /v $exeSource
  if ($LASTEXITCODE -ne 0) {
    throw "signtool.exe failed with exit code $LASTEXITCODE"
  }
}

$signatureStatus = Get-SafeSignatureStatus -Path $exeSource
if (-not $AllowUnsigned -and $signatureStatus -ne "Valid") {
  throw "Launcher signature verification failed: $signatureStatus"
}

Remove-Item $releaseDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $releaseDir | Out-Null
Copy-Item $exeSource $exeTarget -Force

Remove-Item $zipPath -Force -ErrorAction SilentlyContinue
Compress-Archive -Path $exeTarget -DestinationPath $zipPath

$hash = (Get-FileHash $exeTarget -Algorithm SHA256).Hash
Write-Host "Created $zipPath"
Write-Host "Signature status: $signatureStatus"
Write-Host "SHA256: $hash"
