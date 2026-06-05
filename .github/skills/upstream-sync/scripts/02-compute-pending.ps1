<#
.SYNOPSIS
  Compute the pending cherry-pick list with revert-pair detection.

.DESCRIPTION
  Reads the last-synced upstream watermark from origin/main's
  `cherry picked from commit <sha>` trailers, lists commits patch-id-aware
  between watermark and upstream/main (oldest first), detects revert pairs
  within the range and drops them, detects upstream-empty commits and drops
  them, and emits the final pending list as JSON.

  `01-fetch-upstream.ps1` must have been run first (we need upstream/main
  in this clone).

.OUTPUTS
  JSON object on stdout:
  {
    "from": "<old_sha>",
    "to":   "<new_sha>",
    "pending":       [ "<sha>", ... ],
    "dropped_pairs": [ ["<orig>", "<revert>"], ... ],
    "skipped_empty": [ "<sha>", ... ]
  }
#>
[CmdletBinding()]
param()

. "$PSScriptRoot/Common.ps1"

# --- Inlined helpers (single-use; see Common.ps1 comment for why) ----------

function Resolve-FullCommitSha {
    param([Parameter(Mandatory)] [string] $Sha)
    $full = (git rev-parse "$Sha^{commit}" 2>$null)
    if ($LASTEXITCODE -ne 0 -or -not $full) { throw "Could not resolve commit SHA '$Sha'." }
    return $full.Trim()
}

function Get-LastSyncedUpstreamSha {
    # Walks origin/main newest-first, returns the first `(cherry picked from
    # commit <sha>)` trailer whose target is reachable from upstream/main.
    # Capped at 5000 most-recent commits — well beyond any realistic
    # weekly-sync deployment. Resolves the trailer SHA to its full 40-char
    # form before the ancestry check so an abbreviated/ambiguous trailer
    # doesn't silently skip an otherwise-valid watermark candidate.
    $commits = @(git log origin/main --max-count=5000 --grep='cherry picked from commit' --format='%H' 2>$null)
    if ($LASTEXITCODE -ne 0) { throw "git log on origin/main failed while deriving last-synced SHA." }
    foreach ($c in $commits) {
        $body = git log -1 --format='%B' $c 2>$null
        if ($body -match '\(cherry picked from commit ([0-9a-f]{7,40})\)') {
            $fullSha = $null
            try { $fullSha = Resolve-FullCommitSha $matches[1] } catch { continue }
            $null = git merge-base --is-ancestor $fullSha upstream/main 2>$null
            if ($LASTEXITCODE -eq 0) { return $fullSha }
        }
    }
    throw "No 'cherry picked from commit' trailer pointing at upstream/main was found on origin/main (scanned the most recent 5000 commits). The very first sync needs an operator to seed the watermark commit (see SKILL.md - 'First-time sync')."
}

function Get-PendingUpstreamShas {
    # Patch-id-aware oldest-first list of upstream/main commits not yet on
    # origin/main. `--cherry-pick` drops picked-then-reverted pairs by patch
    # ID; -Since further trims by ancestry to keep the walk fast.
    param([string] $Since)
    $out = @(git log --cherry-pick --right-only --no-merges --format='%H' --reverse 'origin/main...upstream/main' 2>$null)
    if ($LASTEXITCODE -ne 0) { throw "git log --cherry-pick failed while computing pending list." }
    $shas = @($out | Where-Object { $_ -match '^[0-9a-f]{40}$' })
    if ($Since) {
        $filtered = New-Object 'System.Collections.Generic.List[string]'
        foreach ($sha in $shas) {
            $null = git merge-base --is-ancestor $sha $Since 2>$null
            if ($LASTEXITCODE -ne 0) { [void] $filtered.Add($sha) }
        }
        $shas = @($filtered)
    }
    return ,$shas
}

# --- Main logic ------------------------------------------------------------

$from = Get-LastSyncedUpstreamSha
$to = (git rev-parse upstream/main).Trim()
if ($LASTEXITCODE -ne 0) { throw "git rev-parse upstream/main failed." }

if ($from -eq $to) {
    @{ from = $from; to = $to; pending = @(); dropped_pairs = @(); skipped_empty = @() } | ConvertTo-Json -Depth 5
    return
}

$all = @(Get-PendingUpstreamShas -Since $from)

# Build sha -> first-line / body lookup (one git invocation per commit is
# fine at typical batch sizes).
$info = @{}
foreach ($sha in $all) {
    $subj = git log -1 --format='%s' $sha
    $body = git log -1 --format='%B' $sha
    $info[$sha] = @{ subject = $subj; body = $body }
}

# Detect revert pairs WITHIN the range. `--cherry-pick` already handled
# pairs that crossed the watermark boundary; this catches same-batch
# original+revert that wasn't on origin/main yet at computation time.
$dropped = New-Object 'System.Collections.Generic.HashSet[string]'
$pairs   = @()
foreach ($sha in $all) {
    if ($dropped.Contains($sha)) { continue }
    $body = $info[$sha].body
    $subj = $info[$sha].subject

    $targetSha = $null
    if ($body -match 'This reverts commit ([0-9a-f]{40})\b') {
        $targetSha = $Matches[1]
    } elseif ($subj -match '^Revert "') {
        # Fallback: match by quoted subject, but only against commits
        # earlier in the oldest-first range, and only when the match is
        # unique (otherwise leave the revert as a normal pick rather
        # than risk dropping the wrong commit).
        $origSubject = $subj -replace '^Revert "', '' -replace '"\s*$', '' -replace '"\.?\s*$',''
        $prefix = @()
        foreach ($candidateSha in $all) {
            if ($candidateSha -eq $sha) { break }
            $prefix += $candidateSha
        }
        $candidates = @($prefix | Where-Object {
            $info[$_].subject -eq $origSubject -and -not $dropped.Contains($_)
        })
        if ($candidates.Count -eq 1) { $targetSha = $candidates[0] }
    }

    if ($targetSha -and $info.ContainsKey($targetSha) -and -not $dropped.Contains($targetSha)) {
        [void] $dropped.Add($sha)
        [void] $dropped.Add($targetSha)
        $pairs += ,@($targetSha, $sha)
    }
}

# Detect upstream-empty (no files touched).
$empty = @()
foreach ($sha in $all) {
    if ($dropped.Contains($sha)) { continue }
    $files = git diff-tree --no-commit-id --name-only -r $sha
    if (-not $files) {
        $empty += $sha
        [void] $dropped.Add($sha)
    }
}

$pending = $all | Where-Object { -not $dropped.Contains($_) }

[ordered] @{
    from          = $from
    to            = $to
    pending       = @($pending)
    dropped_pairs = @($pairs)
    skipped_empty = @($empty)
} | ConvertTo-Json -Depth 5
