<#
.SYNOPSIS
  Orchestrator: run one upstream-sync pass. Safe to invoke from a
  scheduler on a weekly/daily cadence.

.DESCRIPTION
  Reads state.json. If the stuck-lock is set, writes a skipped-locked
  report and exits 0. Otherwise:
    1. Fetches upstream/main.
    2. Computes pending commits, dropping revert pairs and empties.
    3. Creates branch upstream-sync/YYYY-MM-DD.
    4. Cherry-picks one-by-one with Tier-0/Tier-1 auto-resolution.
    5. Writes a report.
    6. On success → pushes branch, opens PR (exit 0).
       On stuck   → pushes branch, opens issue, sets lock (exit 10).
       On no-op   → exits 0 with a "no-op" report.

.PARAMETER DryRun
  Compute & report only; do not create the branch or pick anything.

.PARAMETER TryTier2
  Reserved: enable LLM-assisted Tier-2 conflict resolution (NOT YET IMPLEMENTED).

.PARAMETER Force
  Override the stuck-lock. DANGEROUS — clobbers the in-progress branch.
  Use only when you know the lock is stale.

.PARAMETER MaxPicks
  Cap the number of cherry-picks per run (default: unlimited). Useful for
  smoke-testing the scheduler with a few commits at a time.

.PARAMETER PushDirectToMain
  Skip the PR and fast-forward main directly to the sync branch tip.
  Requires push permission on main (admin / branch-protection bypass).
  Preserves per-commit content, order, and original author dates — strictly
  better than a squash-merged PR. Use when there's no need for a human
  review checkpoint per sync.

.PARAMETER AutoMergeStrategy
  PR mode only. After opening the PR, run `gh pr merge --<strategy> --auto`
  so when CI/approvals pass, GitHub auto-merges with the right strategy.
  Allowed: 'rebase' (preserves per-commit, recommended), 'merge' (adds a
  merge commit; per-commit also preserved), or 'none' (default; human
  picks the strategy manually — but they must NOT pick squash).

.OUTPUTS
  Writes status to stdout. Exit codes:
    0  = success (PR opened) OR no-op OR skipped-locked
    10 = stuck (issue opened, lock set) — NOT an error
    20 = hard failure (git/gh broken) — alarm-worthy
#>
[CmdletBinding()]
param(
    [switch] $DryRun,
    [switch] $TryTier2,
    [switch] $Force,
    [int]    $MaxPicks = 0,
    [switch] $PushDirectToMain,
    [ValidateSet('rebase','merge','none')] [string] $AutoMergeStrategy = 'none'
)

. "$PSScriptRoot/Common.ps1"

function Exit-Hard([string] $msg) {
    Write-Error $msg
    exit 20
}

try {
    $state = Read-State
    $ctx = New-RunContext

    # --- Stuck-lock gate ---
    if ($state.stuck_on_sha -and -not $Force) {
        Write-Host "Stuck-lock set at $($state.stuck_on_sha) (issue: $($state.stuck_issue_url)). Skipping." -ForegroundColor Yellow
        $reportPath = & "$PSScriptRoot/05-write-report.ps1" -Ctx $ctx -From $state.last_synced_upstream_sha -To $state.last_synced_upstream_sha -Status 'skipped-locked'
        Write-Host "Skip report: $reportPath"
        exit 0
    }

    Assert-CleanWorktree
    git switch main 2>&1 | Out-Host
    if ($LASTEXITCODE -ne 0) { Exit-Hard "git switch main failed." }
    git pull --ff-only 2>&1 | Out-Host
    if ($LASTEXITCODE -ne 0) { Exit-Hard "git pull --ff-only main failed." }

    # --- 1. Fetch upstream ---
    $toSha = (& "$PSScriptRoot/01-fetch-upstream.ps1").Trim()
    $fromSha = $state.last_synced_upstream_sha

    if ($toSha -eq $fromSha) {
        Write-Host "Already at upstream HEAD ($toSha). No-op." -ForegroundColor Green
        $reportPath = & "$PSScriptRoot/05-write-report.ps1" -Ctx $ctx -From $fromSha -To $toSha -Status 'no-op'
        Write-Host "No-op report: $reportPath"
        exit 0
    }

    # --- 2. Compute pending ---
    $pendingJson = & "$PSScriptRoot/02-compute-pending.ps1"
    $pending = $pendingJson | ConvertFrom-Json
    Write-Host ("Pending: {0} commits, {1} revert pairs dropped, {2} empties dropped." -f $pending.pending.Count, $pending.dropped_pairs.Count, $pending.skipped_empty.Count)

    $ctx.DroppedPairs = @($pending.dropped_pairs)
    $ctx.SkippedEmpty = @($pending.skipped_empty)

    if ($pending.pending.Count -eq 0) {
        Write-Host "Nothing to pick after filtering. Effective no-op." -ForegroundColor Green
        $reportPath = & "$PSScriptRoot/05-write-report.ps1" -Ctx $ctx -From $fromSha -To $toSha -Status 'no-op'
        Write-Host "Report: $reportPath"
        exit 0
    }

    if ($DryRun) {
        Write-Host "DryRun: skipping branch creation and cherry-picks." -ForegroundColor Cyan
        $reportPath = & "$PSScriptRoot/05-write-report.ps1" -Ctx $ctx -From $fromSha -To $toSha -Status 'no-op'
        Write-Host "DryRun report: $reportPath"
        exit 0
    }

    # --- 3. Create / switch to sync branch ---
    $branch = $ctx.Branch
    git switch -c $branch 2>$null
    if ($LASTEXITCODE -ne 0) {
        git switch $branch 2>&1 | Out-Host
        if ($LASTEXITCODE -ne 0) { Exit-Hard "Could not create or switch to $branch." }
    }

    # --- 4. Cherry-pick loop ---
    $picks = $pending.pending
    if ($MaxPicks -gt 0 -and $picks.Count -gt $MaxPicks) { $picks = $picks[0..($MaxPicks-1)] }

    foreach ($sha in $picks) {
        Write-Host ""
        Write-Host "=== Cherry-pick $sha ===" -ForegroundColor Cyan
        $resJson = & "$PSScriptRoot/03-cherry-pick-one.ps1" -Sha $sha
        $res = $resJson | ConvertFrom-Json
        switch ($res.status) {
            'picked' {
                $ctx.Picked += $sha
                foreach ($p in @($res.tier0_paths)) {
                    $ctx.Tier0 += [pscustomobject] @{ Sha = $sha; Path = $p }
                }
            }
            'skipped-empty' {
                $ctx.SkippedEmpty += $sha
            }
            'stuck' {
                $ctx.StuckSha   = $sha
                $ctx.StuckPaths = @($res.conflict_paths)
                $ctx.Status     = 'stuck'
                Write-Warning "Stuck at $sha on paths: $($res.conflict_paths -join ', ')"
                break
            }
            default { Exit-Hard "Unknown cherry-pick-one status: $($res.status)" }
        }
        if ($ctx.Status -eq 'stuck') { break }
    }

    # --- 5. Report + finalize ---
    if ($ctx.Status -eq 'stuck') {
        $reportPath = & "$PSScriptRoot/05-write-report.ps1" -Ctx $ctx -From $fromSha -To $toSha -Status 'stuck'
        $ctx.ReportPath = $reportPath
        Write-Host "Stuck report: $reportPath"
        $issueUrl = & "$PSScriptRoot/07-open-stuck-issue.ps1" -Ctx $ctx -ReportPath $reportPath
        Write-Host "Stuck issue: $issueUrl" -ForegroundColor Yellow
        exit 10
    }

    $ctx.Status = 'ok'
    $reportPath = & "$PSScriptRoot/05-write-report.ps1" -Ctx $ctx -From $fromSha -To $toSha -Status 'ok'
    $ctx.ReportPath = $reportPath
    Write-Host "Report: $reportPath"

    if ($PushDirectToMain) {
        $mainHead = & "$PSScriptRoot/06b-finalize-direct.ps1" -Ctx $ctx -To $toSha -ReportPath $reportPath
        Write-Host ""
        Write-Host "✅ Sync fast-forwarded onto main at $($mainHead.Substring(0,9))" -ForegroundColor Green
        exit 0
    }

    $prUrl = & "$PSScriptRoot/06-finalize-pr.ps1" -Ctx $ctx -To $toSha -ReportPath $reportPath -AutoMergeStrategy $AutoMergeStrategy
    Write-Host ""
    Write-Host "✅ Sync PR opened: $prUrl" -ForegroundColor Green
    exit 0
}
catch {
    Write-Error $_.Exception.Message
    Write-Error $_.ScriptStackTrace
    exit 20
}
