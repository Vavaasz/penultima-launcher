$ErrorActionPreference = "Stop"

$cargo = Join-Path $env:USERPROFILE ".cargo\\bin\\cargo.exe"
if (-not (Test-Path $cargo)) {
  throw "Cargo was not found at $cargo"
}

$root = "D:\\Server\\Launcher"
$releaseDir = "D:\\Server\\_publish\\penultima-launcher-release"
$zipPath = "D:\\Server\\_publish\\Penultima-Launcher.zip"
$exeSource = Join-Path $root "target\\release\\penultima-launcher.exe"
$exeTarget = Join-Path $releaseDir "penultima-launcher.exe"
$rootExe = Join-Path $root "penultima-launcher.exe"
$clientFeed = "https://github.com/Vavaasz/penultima-client"

& $cargo build --manifest-path (Join-Path $root "Cargo.toml") --release

if (-not (Test-Path $exeSource)) {
  throw "Launcher executable was not produced at $exeSource"
}

Remove-Item $releaseDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $releaseDir | Out-Null
Copy-Item $exeSource $exeTarget -Force
Copy-Item $exeSource $rootExe -Force

Remove-Item $zipPath -Force -ErrorAction SilentlyContinue
Compress-Archive -Path $exeTarget -DestinationPath $zipPath

Write-Host "Created $zipPath"
