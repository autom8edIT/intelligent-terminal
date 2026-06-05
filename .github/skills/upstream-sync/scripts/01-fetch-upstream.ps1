<#
.SYNOPSIS
  Ensure upstream remote exists and fetch upstream/main.
.OUTPUTS
  Writes the current upstream/main SHA to stdout.
#>
[CmdletBinding()]
param(
    [string] $UpstreamUrl = 'https://github.com/microsoft/terminal.git'
)

. "$PSScriptRoot/Common.ps1"

# Ensure-UpstreamRemote (inlined — single-use). Adds the `upstream` remote
# if missing; bails if it points somewhere unexpected.
$existing = git remote get-url upstream 2>$null
if ($LASTEXITCODE -ne 0) {
    git remote add upstream $UpstreamUrl | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Failed to add 'upstream' remote." }
} elseif ($existing.Trim() -ne $UpstreamUrl) {
    throw "Remote 'upstream' points at '$($existing.Trim())' (expected '$UpstreamUrl'). Fix the remote before running upstream-sync."
}

git fetch upstream main --no-tags 2>&1 | Out-Host
if ($LASTEXITCODE -ne 0) { throw "git fetch upstream main failed." }

$sha = git rev-parse upstream/main
if ($LASTEXITCODE -ne 0) { throw "git rev-parse upstream/main failed." }
return $sha.Trim()
