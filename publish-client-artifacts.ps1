param(
  [string]$SourceRoot = "D:\\Server\\Cliente-15.23-Prod",
  [string]$PublicFeedRoot = "D:\\Server\\_publish\\penultima-client",
  [string]$WebsiteRoot = "D:\\Server\\UniServerZ\\www",
  [string]$Version = "",
  [string]$SourceCommit = "",
  [switch]$SkipPublicFeed,
  [switch]$SkipPublicFeedPush,
  [switch]$SkipWebsite
)

$ErrorActionPreference = "Stop"

function Assert-ExternalSuccess([string]$Label) {
  if ($LASTEXITCODE -ne 0) {
    throw "$Label failed with exit code $LASTEXITCODE"
  }
}

function Assert-PathExists([string]$Path, [string]$Label) {
  if (-not (Test-Path -LiteralPath $Path)) {
    throw "$Label not found: $Path"
  }
}

function Resolve-ClientPublishVersion(
  [string]$RepositoryRoot,
  [string]$RequestedVersion,
  [string]$CommitRef
) {
  if (-not [string]::IsNullOrWhiteSpace($RequestedVersion)) {
    return $RequestedVersion.Trim()
  }

  $resolvedCommitRef = if ([string]::IsNullOrWhiteSpace($CommitRef)) { "HEAD" } else { $CommitRef }
  $shortCommit = (& git -C $RepositoryRoot rev-parse --short=12 $resolvedCommitRef).Trim()
  Assert-ExternalSuccess "Resolve client publish version"

  if (-not [string]::IsNullOrWhiteSpace($shortCommit)) {
    return "15.23-prod-$shortCommit"
  }

  return "15.23-prod-$((Get-Date).ToUniversalTime().ToString('yyyyMMddHHmmss'))"
}

$publishClientFeedScript = Join-Path $PSScriptRoot "publish-client-feed.ps1"
$publishWebsiteScript = Join-Path $SourceRoot "sounds\\publish-website-client-assets.ps1"
$resolvedCommitRef = if ([string]::IsNullOrWhiteSpace($SourceCommit)) { "HEAD" } else { $SourceCommit }
$resolvedVersion = Resolve-ClientPublishVersion -RepositoryRoot $SourceRoot -RequestedVersion $Version -CommitRef $resolvedCommitRef

Assert-PathExists -Path $SourceRoot -Label "Client root"
Assert-PathExists -Path $publishClientFeedScript -Label "Public client feed publish script"

if (-not $SkipPublicFeed) {
  & $publishClientFeedScript `
    -SourceRoot $SourceRoot `
    -OutputRoot $PublicFeedRoot `
    -Version $resolvedVersion `
    -CommitAndPush `
    -SourceCommit $resolvedCommitRef `
    -SkipPush:$SkipPublicFeedPush
  Assert-ExternalSuccess "Publish public client feed"
}

if (-not $SkipWebsite) {
  Assert-PathExists -Path $publishWebsiteScript -Label "Website client publish script"

  & $publishWebsiteScript `
    -ClientRoot $SourceRoot `
    -WebsiteRoot $WebsiteRoot `
    -Version $resolvedVersion `
    -RebuildMetadata
  Assert-ExternalSuccess "Publish website client downloads"
}

Write-Host "Published client artifacts for $resolvedCommitRef as $resolvedVersion."
