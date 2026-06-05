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
# if missing; bails if it points somewhere unexpected. Normalizes common URL
# variants (https vs ssh, with/without trailing `.git`, with/without trailing
# slash) so the same logical repo configured under any of those forms is
# accepted.
function _NormalizeRemoteUrl {
    param([string] $Url)
    $u = $Url.Trim().TrimEnd('/').ToLowerInvariant()
    # git@github.com:owner/repo(.git)? -> github.com/owner/repo
    if ($u -match '^git@([^:]+):(.+?)(\.git)?$') {
        $u = "$($Matches[1])/$($Matches[2])"
    }
    # https://host/owner/repo(.git)? -> host/owner/repo
    elseif ($u -match '^https?://([^/]+)/(.+?)(\.git)?$') {
        $u = "$($Matches[1])/$($Matches[2])"
    }
    return $u
}

$existing = git remote get-url upstream 2>$null
if ($LASTEXITCODE -ne 0) {
    git remote add upstream $UpstreamUrl | Out-Null
    if ($LASTEXITCODE -ne 0) { throw "Failed to add 'upstream' remote." }
} elseif ((_NormalizeRemoteUrl $existing) -ne (_NormalizeRemoteUrl $UpstreamUrl)) {
    throw "Remote 'upstream' points at '$($existing.Trim())' (expected '$UpstreamUrl'). Fix the remote before running upstream-sync."
}

git fetch upstream main --no-tags 2>&1 | ForEach-Object { [Console]::Error.WriteLine($_) }
if ($LASTEXITCODE -ne 0) { throw "git fetch upstream main failed." }

$sha = git rev-parse upstream/main
if ($LASTEXITCODE -ne 0) { throw "git rev-parse upstream/main failed." }
return $sha.Trim()
