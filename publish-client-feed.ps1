$ErrorActionPreference = "Stop"

$cargo = Join-Path $env:USERPROFILE ".cargo\\bin\\cargo.exe"
if (-not (Test-Path $cargo)) {
  throw "Cargo was not found at $cargo"
}

$source = "D:\\Server\\Cliente-15.23-Prod"
$output = "D:\\Server\\_publish\\penultima-client"
$version = "15.23-prod-" + (Get-Date -Format "yyyyMMdd-HHmmss")
$manifest = "D:\\Server\\Launcher\\tools\\client-feed-builder\\Cargo.toml"

& $cargo run --manifest-path $manifest --release -- --source $source --output $output --version $version
