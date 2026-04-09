$ErrorActionPreference = "Stop"

$sourceRoot = "D:\\Server\\Cliente-15.23-Prod"
$hookPath = Join-Path $sourceRoot ".git\\hooks\\post-commit"
$publishScript = "D:/Server/Launcher/publish-client-feed.ps1"
$logPath = "D:/Server/Cliente-15.23-Prod/.git/penultima-public-feed.log"

$hook = @'
#!/bin/sh
command -v git-lfs >/dev/null 2>&1 || { printf >&2 "\n%s\n\n" "This repository is configured for Git LFS but 'git-lfs' was not found on your path. If you no longer wish to use Git LFS, remove this hook by deleting the 'post-commit' file in the hooks directory (set by 'core.hookspath'; usually '.git/hooks')."; exit 2; }
git lfs post-commit "$@"

repo_root="$(git rev-parse --show-toplevel)"
commit_short="$(git rev-parse --short HEAD)"
log_file="__LOG_PATH__"

printf "\n[%s] Publishing public client feed from %s\n" "$(date '+%Y-%m-%d %H:%M:%S')" "$commit_short" >> "$log_file"
powershell.exe -NoProfile -ExecutionPolicy Bypass -File "__PUBLISH_SCRIPT__" -CommitAndPush -SourceCommit "$commit_short" >> "$log_file" 2>&1 || {
  printf "Public client feed publish failed for %s\n" "$commit_short" >> "$log_file"
}
'@

$hook = $hook.Replace("__LOG_PATH__", $logPath.Replace("\", "/"))
$hook = $hook.Replace("__PUBLISH_SCRIPT__", $publishScript)

Set-Content -Path $hookPath -Value $hook -Encoding Ascii -NoNewline
Write-Host "Installed post-commit publish hook at $hookPath"
