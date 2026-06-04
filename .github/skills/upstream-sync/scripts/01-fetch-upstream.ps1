<#
.SYNOPSIS
  Ensure upstream remote exists and fetch upstream/main.
.OUTPUTS
  Writes the current upstream/main SHA to stdout.
#>
[CmdletBinding()]
param()

. "$PSScriptRoot/Common.ps1"

Ensure-UpstreamRemote
git fetch upstream main --no-tags 2>&1 | Out-Host
if ($LASTEXITCODE -ne 0) { throw "git fetch upstream main failed." }

$sha = git rev-parse upstream/main
if ($LASTEXITCODE -ne 0) { throw "git rev-parse upstream/main failed." }
return $sha.Trim()
