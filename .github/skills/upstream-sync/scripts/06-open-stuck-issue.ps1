<#
.SYNOPSIS
  Open a Tier-3 stuck issue (cherry-pick stopped at a real merge conflict).

.DESCRIPTION
  The "lock" IS this open issue — the next sync run queries
  `gh issue list --label upstream-sync-stuck --state open` and bails when
  one exists. The issue body carries a fenced ```yaml # wta-state``` block
  with enough metadata to recognize the same failure on a re-run.

  Cleared by: a human closes the issue. That's it.

.PARAMETER Branch
  Stuck sync branch name.

.PARAMETER StuckSha
  The upstream SHA the pick got stuck on.

.PARAMETER ConflictPaths
  Files that 03-cherry-pick-one.ps1 left unmerged.

.PARAMETER StuckError
  Optional error string captured from 03 (e.g. "git cherry-pick exited 1
  with no conflict paths" for a non-conflict failure).

.PARAMETER ExtraBody
  Optional markdown to append after the YAML block (resume instructions,
  log excerpts, etc.).

.OUTPUTS
  Issue URL on stdout.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string]   $Branch,
    [Parameter(Mandatory)] [string]   $StuckSha,
    [string[]] $ConflictPaths = @(),
    [string]   $StuckError    = '',
    [string]   $ExtraBody     = ''
)

. "$PSScriptRoot/Common.ps1"

# Push the stuck branch so the human can resume on it.
git push -u origin $Branch 2>&1 | Out-Host
if ($LASTEXITCODE -ne 0) { Write-Warning "Could not push stuck branch — issue still being filed for visibility." }

$shortSha    = $StuckSha.Substring(0,9)
$subj        = (git log -1 --format='%s' $StuckSha).Trim()
$author      = (git log -1 --format='%an <%ae>' $StuckSha).Trim()
$upstreamUrl = "https://github.com/microsoft/terminal/commit/$StuckSha"
$title       = "Upstream sync stuck at ${shortSha}: $subj"

$yamlBlock = Format-StuckYamlBlock @{
    tier           = '3'
    kind           = 'cherry-pick-conflict'
    stuck_on_sha   = $StuckSha
    branch         = $Branch
    at             = Format-Iso8601
    host           = $env:COMPUTERNAME
    conflict_count = $ConflictPaths.Count
    error          = $StuckError
}

$conflictList = if ($ConflictPaths.Count -gt 0) {
    ($ConflictPaths | ForEach-Object { "- ``$_``" }) -join "`n"
} else {
    '_(no conflict paths reported — see error)_'
}

$body = @"
> [!CAUTION]
> **Upstream sync stopped at a conflict that needs human judgment.**
>
> The scheduler will keep skipping its runs until this issue is **closed**.
> Closing the issue IS the lock-clear signal — no separate script needed.

**Stuck on:** ``$StuckSha`` — $subj
**Upstream commit:** [$shortSha]($upstreamUrl)
**Author:** $author
**Sync branch:** ``$Branch`` (push attempted — run ``git ls-remote --heads origin $Branch`` to verify it landed)

**Conflicting paths:**
$conflictList

$yamlBlock

---

$ExtraBody
"@

$tmp = New-TemporaryFile
[System.IO.File]::WriteAllText($tmp, $body, (New-Object System.Text.UTF8Encoding($false)))

# Ensure label exists (best-effort). -R pinned because an `upstream` remote
# can make `gh` default to microsoft/terminal where this account has no
# label-create permission.
gh label create 'upstream-sync-stuck' --color 'B60205' --description 'Upstream sync blocked on a manual conflict' -R microsoft/intelligent-terminal 2>$null | Out-Null

# Capture stderr to a separate temp file so a `gh` warning on stderr can't
# displace the URL as the last line of merged output.
$errFile  = [System.IO.Path]::GetTempFileName()
$errText  = ''
$issueUrl = $null
$ghExit   = 0
try {
    $issueUrl = gh issue create -R microsoft/intelligent-terminal --title $title --label 'upstream-sync-stuck' --body-file $tmp 2>$errFile | Select-Object -Last 1
    $ghExit   = $LASTEXITCODE
    if (Test-Path -LiteralPath $errFile) { $errText = (Get-Content -Raw -LiteralPath $errFile) }
}
finally {
    Remove-Item -LiteralPath $tmp     -Force -ErrorAction SilentlyContinue
    Remove-Item -LiteralPath $errFile -Force -ErrorAction SilentlyContinue
}
if ($ghExit -ne 0 -or $issueUrl -notmatch '^https://github.com/') {
    throw "gh issue create failed (exit $ghExit): stdout='$issueUrl' stderr='$errText'"
}

return $issueUrl.Trim()
