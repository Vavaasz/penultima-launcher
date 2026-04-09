param(
  [string]$Version,
  [string]$SourceRoot = "D:\\Server\\Cliente-15.23-Prod",
  [string]$OutputRoot = "D:\\Server\\_publish\\penultima-client",
  [switch]$CommitAndPush,
  [string]$SourceCommit = "",
  [switch]$SkipPush
)

$ErrorActionPreference = "Stop"

function Assert-ExternalSuccess([string]$Label) {
  if ($LASTEXITCODE -ne 0) {
    throw "$Label failed with exit code $LASTEXITCODE"
  }
}

function New-CommitSnapshot([string]$RepositoryRoot, [string]$CommitRef, [string]$GitExecutable) {
  $snapshotRoot = Join-Path ([System.IO.Path]::GetTempPath()) ("penultima-client-feed-" + [guid]::NewGuid().ToString("N"))
  $archivePath = "${snapshotRoot}.zip"

  New-Item -ItemType Directory -Path $snapshotRoot | Out-Null
  & $GitExecutable -C $RepositoryRoot archive --format=zip -o $archivePath $CommitRef
  Assert-ExternalSuccess "Create source snapshot"
  Expand-Archive -Path $archivePath -DestinationPath $snapshotRoot -Force
  Remove-Item $archivePath -Force -ErrorAction SilentlyContinue
  return $snapshotRoot
}

$cargo = Join-Path $env:USERPROFILE ".cargo\\bin\\cargo.exe"
if (-not (Test-Path $cargo)) {
  throw "Cargo was not found at $cargo"
}

$git = "git"
$manifest = "D:\\Server\\Launcher\\tools\\client-feed-builder\\Cargo.toml"
$resolvedVersion = if ($Version) { $Version } else { "auto" }
$builderSource = $SourceRoot
$snapshotRoot = $null

if ($SourceCommit) {
  $snapshotRoot = New-CommitSnapshot -RepositoryRoot $SourceRoot -CommitRef $SourceCommit -GitExecutable $git
  $builderSource = $snapshotRoot
}

try {
  & $cargo run --manifest-path $manifest --release -- --source $builderSource --output $OutputRoot --version $resolvedVersion
  Assert-ExternalSuccess "Client feed build"

  if (-not $CommitAndPush) {
    Write-Host "Generated public feed only."
    exit 0
  }

  if (-not (Test-Path (Join-Path $OutputRoot ".git"))) {
    throw "Output root is not a git repository: $OutputRoot"
  }

  $statusLines = @(& $git -C $OutputRoot status --short)
  Assert-ExternalSuccess "Git status"
  if (-not $statusLines -or ($statusLines | Measure-Object).Count -eq 0) {
    Write-Host "Public client feed already matches current source state."
    exit 0
  }

  & $git -C $OutputRoot add --all
  Assert-ExternalSuccess "Git add"

  $shortCommit = if ($SourceCommit) {
    $SourceCommit
  } else {
    (& $git -C $SourceRoot rev-parse --short HEAD).Trim()
  }
  Assert-ExternalSuccess "Resolve source commit"

  $commitMessage = if ($shortCommit) {
    "Publish client feed from source $shortCommit"
  } else {
    "Publish client feed update"
  }

  & $git -C $OutputRoot commit -m $commitMessage
  Assert-ExternalSuccess "Git commit"

  if ($SkipPush) {
    Write-Host "Committed public client feed locally without push."
    exit 0
  }

  & $git -C $OutputRoot push origin HEAD
  Assert-ExternalSuccess "Git push"
  Write-Host "Published public client feed to origin."
}
finally {
  if ($snapshotRoot -and (Test-Path $snapshotRoot)) {
    Remove-Item $snapshotRoot -Recurse -Force -ErrorAction SilentlyContinue
  }
}
