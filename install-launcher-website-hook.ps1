$ErrorActionPreference = "Stop"

$launcherRoot = "D:\\Server\\Launcher"
$hookPath = Join-Path $launcherRoot ".git\\hooks\\post-commit"
$publishScript = "D:/Server/Launcher/deploy-website-downloads.ps1"
$clientRoot = "D:/Server/Cliente-15.23-Prod"
$websiteRoot = "D:/Server/UniServerZ/www"
$logPath = "D:/Server/Launcher/.git/penultima-launcher-website-publish.log"

$hook = @'
#!/bin/sh
repo_root="$(git rev-parse --show-toplevel)"
commit_short="$(git rev-parse --short HEAD)"
log_file="__LOG_PATH__"

unsigned_arg=""
if [ -z "$PENULTIMA_SIGN_CERT_THUMBPRINT" ] && [ -z "$PENULTIMA_SIGN_CERT_PATH" ]; then
  unsigned_arg="-AllowUnsignedLauncher"
fi

printf "\n[%s] Publishing website launcher download from %s\n" "$(date '+%Y-%m-%d %H:%M:%S')" "$commit_short" >> "$log_file"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "__PUBLISH_SCRIPT__" -LauncherRoot "$repo_root" -ClientRoot "__CLIENT_ROOT__" -WebsiteRoot "__WEBSITE_ROOT__" -SkipClient $unsigned_arg >> "$log_file" 2>&1 || {
  printf "Launcher website publish failed for %s\n" "$commit_short" >> "$log_file"
}
'@

$hook = $hook.Replace("__LOG_PATH__", $logPath.Replace("\", "/"))
$hook = $hook.Replace("__PUBLISH_SCRIPT__", $publishScript)
$hook = $hook.Replace("__CLIENT_ROOT__", $clientRoot)
$hook = $hook.Replace("__WEBSITE_ROOT__", $websiteRoot)

Set-Content -Path $hookPath -Value $hook -Encoding Ascii -NoNewline
Write-Host "Installed launcher website post-commit hook at $hookPath"
