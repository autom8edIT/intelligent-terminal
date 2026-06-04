<#
.SYNOPSIS
  Clear the stuck-lock after a human has merged the manual-resolution PR.

.DESCRIPTION
  Validates that -ResolvedThroughSha is an ancestor of upstream/main and
  is at least as new as the stuck SHA, then advances last_synced to it
  and clears stuck_on_sha / stuck_branch / stuck_at / stuck_issue_url.
  Commits state.json on main.

.PARAMETER ResolvedThroughSha
  The upstream SHA the manual-resolution PR brought the fork up to.
  This becomes the new last_synced_upstream_sha. Typically this is the
  same SHA that was stuck — the next scheduled run picks up from
  ResolvedThroughSha + 1.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $ResolvedThroughSha
)

. "$PSScriptRoot/Common.ps1"

$state = Read-State
if (-not $state.stuck_on_sha) {
    Write-Warning "No stuck-lock is set. Nothing to clear."
    return
}

Ensure-UpstreamRemote
git fetch upstream main --no-tags | Out-Null

# Validate the new SHA is on upstream/main.
$null = git merge-base --is-ancestor $ResolvedThroughSha upstream/main
if ($LASTEXITCODE -ne 0) {
    throw "ResolvedThroughSha $ResolvedThroughSha is not on upstream/main. Refusing to clear lock."
}

# Validate it's >= the stuck SHA (i.e., stuck is ancestor of resolved).
$null = git merge-base --is-ancestor $state.stuck_on_sha $ResolvedThroughSha
if ($LASTEXITCODE -ne 0) {
    throw "stuck_on_sha $($state.stuck_on_sha) is not an ancestor of $ResolvedThroughSha. Refusing — pass the same SHA or a later one."
}

git switch main | Out-Null
git pull --ff-only | Out-Null

$state.last_synced_upstream_sha = $ResolvedThroughSha
$state.stuck_on_sha    = $null
$state.stuck_branch    = $null
$state.stuck_at        = $null
$state.stuck_issue_url = $null
Write-State $state

git add -- (Get-StatePath) | Out-Null
git commit -m "chore(upstream-sync): clear stuck-lock at $($ResolvedThroughSha.Substring(0,9))" | Out-Host
if ($LASTEXITCODE -ne 0) { throw "git commit failed (state unchanged?); lock is NOT cleared on origin/main." }

git push origin main | Out-Host
if ($LASTEXITCODE -ne 0) { throw "git push origin main failed — lock cleared locally only. Push manually before the next scheduler tick." }
Write-Host "Stuck-lock cleared. Next scheduled run will resume from $($ResolvedThroughSha.Substring(0,9))+1." -ForegroundColor Green
