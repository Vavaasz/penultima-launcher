$ErrorActionPreference = "Stop"

$cargo = Join-Path $env:USERPROFILE ".cargo\\bin\\cargo.exe"
if (-not (Test-Path $cargo)) {
  throw "Cargo was not found at $cargo"
}

$root = "D:\\Server\\Launcher"
$releaseDir = "D:\\Server\\_publish\\penultima-launcher-release"
$zipPath = "D:\\Server\\_publish\\Penultima-Launcher.zip"
$exeSource = Join-Path $root "target\\release\\ultima-launcher.exe"
$exeTarget = Join-Path $releaseDir "ultima-launcher.exe"
$rootExe = Join-Path $root "ultima-launcher.exe"
$clientFeed = "https://github.com/Vavaasz/penultima-client"
$version = (Get-Date -Format "yyyyMMdd-HHmmss")

& $cargo build --manifest-path (Join-Path $root "Cargo.toml") --release

if (-not (Test-Path $exeSource)) {
  throw "Launcher executable was not produced at $exeSource"
}

Remove-Item $releaseDir -Recurse -Force -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Path $releaseDir | Out-Null
Copy-Item $exeSource $exeTarget -Force
Copy-Item $exeSource $rootExe -Force

$readme = @"
Penultima Launcher
==================

1. Run ultima-launcher.exe
2. Let the launcher download or update the client automatically
3. Default public client feed: $clientFeed
4. Build stamp: $version
"@

Set-Content -Path (Join-Path $releaseDir "README.txt") -Value $readme -Encoding ASCII

Remove-Item $zipPath -Force -ErrorAction SilentlyContinue
Compress-Archive -Path (Join-Path $releaseDir "*") -DestinationPath $zipPath

Write-Host "Created $zipPath"
