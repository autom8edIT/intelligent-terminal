<#
.SYNOPSIS
  Push the sync branch and open a PR. No state file, no extra commits.

.DESCRIPTION
  The branch already carries the cherry-picked commits (each with its
  `(cherry picked from commit <sha>)` trailer — that IS the watermark
  the next run reads). We just push it and open the PR.

  Called by the agent after a clean cherry-pick batch (and build pass).

.PARAMETER Branch
  Sync branch name (must already exist locally and be checked out / pushable).

.PARAMETER UpstreamHeadSha
  Upstream/main SHA at fetch time. Used only in the PR title.

.PARAMETER PickedCount
  Number of commits cherry-picked in this batch. Used only in the banner.

.PARAMETER PrBody
  Full markdown body for the PR. The banner (squash-warning + review-fix
  policy) is prepended automatically.

.PARAMETER AutoMergeStrategy
  rebase | merge | none. Passed to `gh pr merge --auto`.

.OUTPUTS
  PR URL on stdout.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Branch,
    [Parameter(Mandatory)] [string] $UpstreamHeadSha,
    [Parameter(Mandatory)] [int]    $PickedCount,
    [Parameter(Mandatory)] [string] $PrBody,
    [ValidateSet('rebase','merge','none')] [string] $AutoMergeStrategy = 'none'
)

. "$PSScriptRoot/Common.ps1"

$banner = @"
> [!WARNING]
> **DO NOT squash-merge this PR.** Squashing collapses every cherry-picked
> upstream commit into one, destroying per-commit attribution, original
> author dates, the ``(cherry picked from commit <sha>)`` trailers that the
> NEXT upstream sync uses as its watermark, and ``git bisect`` resolution.
> Merge with **"Rebase and merge"** (preferred — flat history, all
> $PickedCount commit(s) land individually) or **"Create a merge commit"**.

> [!NOTE]
> **Review-fix policy.** Only build-blocking fixes belong on this branch
> as **one** focused extra commit. All other Copilot / human review
> feedback goes into a **follow-up PR** based on this PR's head. See
> [``.github/skills/upstream-sync/references/follow-up-pr.md``](https://github.com/microsoft/intelligent-terminal/blob/main/.github/skills/upstream-sync/references/follow-up-pr.md).

---

"@

$bodyPath = New-TemporaryFile
$bodyContent = $banner + $PrBody
[System.IO.File]::WriteAllText($bodyPath, $bodyContent, (New-Object System.Text.UTF8Encoding($false)))

$shortTo = $UpstreamHeadSha.Substring(0,9)
$title = "chore(upstream): sync microsoft/terminal up to $shortTo"

git push -u origin $Branch 2>&1 | ForEach-Object { [Console]::Error.WriteLine($_) }
if ($LASTEXITCODE -ne 0) {
    Remove-Item -LiteralPath $bodyPath -Force -ErrorAction SilentlyContinue
    throw "git push failed for $Branch."
}

# `gh pr create` on Windows occasionally fails with "Head sha can't be blank"
# right after a push — retry up to 3x with a short delay. stderr goes to a
# separate temp file so a `gh` version-update notice can't displace the
# URL as the "last line" of merged output.
$prUrl   = $null
$errFile = [System.IO.Path]::GetTempFileName()
try {
    for ($attempt = 1; $attempt -le 3; $attempt++) {
        Set-Content -LiteralPath $errFile -Value '' -NoNewline
        $prUrl = gh pr create -R microsoft/intelligent-terminal --base main --head $Branch --title $title --body-file $bodyPath 2>$errFile | Select-Object -Last 1
        if ($LASTEXITCODE -eq 0 -and $prUrl -match '^https://github.com/') { break }
        $errText = if (Test-Path -LiteralPath $errFile) { (Get-Content -Raw -LiteralPath $errFile) } else { '' }
        Write-Warning "gh pr create attempt $attempt failed (exit $LASTEXITCODE): stdout='$prUrl' stderr='$errText'"
        Start-Sleep -Seconds 5
    }
    if ($LASTEXITCODE -ne 0 -or $prUrl -notmatch '^https://github.com/') {
        $errText = if (Test-Path -LiteralPath $errFile) { (Get-Content -Raw -LiteralPath $errFile) } else { '' }
        throw "gh pr create did not return a PR URL after 3 attempts. Last stdout: '$prUrl'. Last stderr: '$errText'."
    }
}
finally {
    Remove-Item -LiteralPath $bodyPath -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $errFile  -Force -ErrorAction SilentlyContinue
}

$prUrl = $prUrl.Trim()

if ($AutoMergeStrategy -ne 'none') {
    $strategyFlag = "--$AutoMergeStrategy"
    gh pr merge -R microsoft/intelligent-terminal $prUrl $strategyFlag --auto --delete-branch 2>&1 | ForEach-Object { [Console]::Error.WriteLine($_) }
    if ($LASTEXITCODE -ne 0) {
        Write-Warning "gh pr merge --auto failed. PR is open at $prUrl; merge manually with '$AutoMergeStrategy' strategy (NOT squash)."
    } else {
        [Console]::Error.WriteLine("Auto-merge armed with strategy: $AutoMergeStrategy")
    }
}

return $prUrl
