<#
.SYNOPSIS
  Generate a sync run report markdown file.

.PARAMETER Ctx
  The run-context hashtable built by 04-run-batch.ps1.

.PARAMETER From
  Baseline upstream SHA before the run.

.PARAMETER To
  Upstream HEAD SHA at fetch time.

.PARAMETER Status
  ok | no-op | stuck | skipped-locked

.OUTPUTS
  Absolute path to the written report file.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] $Ctx,
    [Parameter(Mandatory)] [string] $From,
    [Parameter(Mandatory)] [string] $To,
    [Parameter(Mandatory)] [ValidateSet('ok','no-op','stuck','skipped-locked')] [string] $Status
)

. "$PSScriptRoot/Common.ps1"

$started = $Ctx.StartedAt
$ended   = Get-Date
$dur     = $ended - $started
$durStr  = "{0}m {1}s" -f [int]$dur.TotalMinutes, ($dur.Seconds)

function Get-Subj([string] $sha) {
    if (-not $sha) { return '' }
    try { return (git log -1 --format='%s' $sha 2>$null) } catch { return '' }
}

$fromSubj = Get-Subj $From
$toSubj   = Get-Subj $To

$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("# Upstream sync — $Status — $(Format-Iso8601 $started)")
$lines.Add("")
$lines.Add("**Status:** $Status  ")
$lines.Add("**Host:** $($Ctx.Host)  ")
$lines.Add("**Duration:** $durStr  ")
$lines.Add("**Baseline (before run):** ``$From`` — $fromSubj  ")
$lines.Add("**Upstream HEAD:** ``$To`` — $toSubj  ")
$lines.Add("**Branch:** ``$($Ctx.Branch)``  ")
$lines.Add("")
$lines.Add("## Summary")
$lines.Add("")
$lines.Add("- Commits picked: **$($Ctx.Picked.Count)**")
$lines.Add("- Revert pairs dropped: **$($Ctx.DroppedPairs.Count)** (= $($Ctx.DroppedPairs.Count * 2) commits skipped, net zero)")
$lines.Add("- Upstream-empty commits skipped: **$($Ctx.SkippedEmpty.Count)**")
$lines.Add("- Tier-0 auto-resolutions: **$($Ctx.Tier0.Count)**")
$lines.Add("- Tier-2 LLM resolutions: **$($Ctx.Tier2.Count)**")
if ($Ctx.StuckSha) {
    $lines.Add("- Tier-3 stuck at: ``$($Ctx.StuckSha)``")
}
$lines.Add("")

if ($Ctx.Picked.Count -gt 0) {
    $lines.Add("## Picked commits (oldest → newest)")
    $lines.Add("")
    $lines.Add("| # | SHA | Subject | Author |")
    $lines.Add("|---|---|---|---|")
    $i = 0
    foreach ($sha in $Ctx.Picked) {
        $i++
        $s = (git log -1 --format='%s' $sha) -replace '\|','\|'
        $a = git log -1 --format='%an' $sha
        $lines.Add("| $i | ``$($sha.Substring(0,9))`` | $s | $a |")
    }
    $lines.Add("")
}

if ($Ctx.DroppedPairs.Count -gt 0) {
    $lines.Add("## Dropped revert pairs")
    $lines.Add("")
    $lines.Add("| Original SHA | Original subject | Revert SHA |")
    $lines.Add("|---|---|---|")
    foreach ($pair in $Ctx.DroppedPairs) {
        $os = (git log -1 --format='%s' $pair[0]) -replace '\|','\|'
        $lines.Add("| ``$($pair[0].Substring(0,9))`` | $os | ``$($pair[1].Substring(0,9))`` |")
    }
    $lines.Add("")
}

if ($Ctx.SkippedEmpty.Count -gt 0) {
    $lines.Add("## Empty / no-op commits skipped")
    $lines.Add("")
    $lines.Add("| SHA | Subject |")
    $lines.Add("|---|---|")
    foreach ($sha in $Ctx.SkippedEmpty) {
        $s = (git log -1 --format='%s' $sha) -replace '\|','\|'
        $lines.Add("| ``$($sha.Substring(0,9))`` | $s |")
    }
    $lines.Add("")
}

if ($Ctx.Tier0.Count -gt 0) {
    $lines.Add("## Tier-0 auto-resolutions")
    $lines.Add("")
    $lines.Add("| Commit SHA | File |")
    $lines.Add("|---|---|")
    foreach ($r in $Ctx.Tier0) {
        $lines.Add("| ``$($r.Sha.Substring(0,9))`` | ``$($r.Path)`` |")
    }
    $lines.Add("")
}

if ($Status -eq 'stuck' -and $Ctx.StuckSha) {
    $stuckSubj = Get-Subj $Ctx.StuckSha
    $stuckAuthor = git log -1 --format='%an <%ae>' $Ctx.StuckSha
    $lines.Add("## Conflict diagnostics")
    $lines.Add("")
    $lines.Add("**Conflicting commit:** [`$($Ctx.StuckSha)`](https://github.com/microsoft/terminal/commit/$($Ctx.StuckSha)) — $stuckSubj  ")
    $lines.Add("**Author:** $stuckAuthor")
    $lines.Add("")
    $lines.Add("**Files in conflict:**")
    $lines.Add("")
    foreach ($p in $Ctx.StuckPaths) { $lines.Add("- ``$p``") }
    $lines.Add("")
    $lines.Add("**Pickup branch:** ``$($Ctx.Branch)`` (pushed to origin)")
    $lines.Add("")
    $lines.Add("**How to resume:**")
    $lines.Add("")
    $lines.Add("1. ``git switch $($Ctx.Branch)``")
    $lines.Add("2. Manually cherry-pick the stuck commit and resolve:")
    $lines.Add("   ``````")
    $lines.Add("   git cherry-pick -x $($Ctx.StuckSha)")
    $lines.Add("   # resolve conflicts, then:")
    $lines.Add("   git add -A && git cherry-pick --continue")
    $lines.Add("   ``````")
    $lines.Add("3. Push and open a PR titled ``chore(upstream-sync): manual resolution for $($Ctx.StuckSha.Substring(0,9))``, merge it.")
    $lines.Add("4. Clear the lock:")
    $lines.Add("   ``````")
    $lines.Add("   pwsh .github/skills/upstream-sync/scripts/clear-stuck.ps1 -ResolvedThroughSha $($Ctx.StuckSha)")
    $lines.Add("   ``````")
    $lines.Add("5. The next scheduled sync resumes from the commit after this one.")
    $lines.Add("")
}

$lines.Add("---")
$lines.Add("")
$lines.Add("_Generated by ``.github/skills/upstream-sync/scripts/05-write-report.ps1``._")

$suffix = if ($Status -eq 'skipped-locked') { 'skipped' } elseif ($Status -eq 'stuck') { 'stuck' } elseif ($Status -eq 'no-op') { 'noop' } else { '' }
$name = Format-ReportFilename -When $started -Suffix $suffix
$path = Join-Path (Get-ReportsDir) $name
[System.IO.File]::WriteAllText($path, ($lines -join "`n"), (New-Object System.Text.UTF8Encoding($false)))
return $path
