<#
.SYNOPSIS
  Open a Tier-4 stuck issue (build failed after a clean cherry-pick batch).

.DESCRIPTION
  Counterpart to 06-open-stuck-issue.ps1 (Tier-3 = mid-pick conflict).
  Tier-4 means all picks completed cleanly but `04-try-build.ps1` said NO
  and the agent couldn't auto-fix.

  Same lock model: the open issue IS the lock; closing it clears.

.PARAMETER Branch
  Sync branch name.

.PARAMETER Kind
  'build-failed' or 'build-inconclusive'.

.PARAMETER PickedCount
  How many commits landed cleanly before the build failed.

.PARAMETER BuildExitCode
  Exit code from 04-try-build.ps1.

.PARAMETER BuildLogTail
  Tail of the build log to embed in the issue body (last ~200 lines).

.PARAMETER BuildLogPath
  Repo-relative path to the full build log (gitignored — for operator
  reference, not committed).

.OUTPUTS
  Issue URL on stdout.
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $Branch,
    [Parameter(Mandatory)] [ValidateSet('build-failed','build-inconclusive')] [string] $Kind,
    [Parameter(Mandatory)] [int]    $PickedCount,
    [Parameter(Mandatory)] [int]    $BuildExitCode,
    [string] $BuildLogTail = '',
    [string] $BuildLogPath = ''
)

. "$PSScriptRoot/Common.ps1"

# Findings hash — stable across runs of the same broken batch so a future
# scheduler tick can recognize "same failure as last time" and skip
# re-opening. Inlined (single use).
function Get-FindingsHash {
    param([Parameter(Mandatory)] $Findings)
    $norm = ($Findings | ConvertTo-Json -Depth 8 -Compress)
    $sha  = [System.Security.Cryptography.SHA256]::Create()
    try {
        $hash = $sha.ComputeHash([System.Text.Encoding]::UTF8.GetBytes($norm))
        return ([System.BitConverter]::ToString($hash) -replace '-','').ToLowerInvariant().Substring(0,16)
    } finally { $sha.Dispose() }
}

$findingsForHash = if ($Kind -eq 'build-failed') {
    @([ordered] @{ exit_code = $BuildExitCode; tail_excerpt = ($BuildLogTail -split "`n" | Select-Object -Last 20) -join "`n" })
} else {
    @([ordered] @{ kind = 'inconclusive'; exit_code = $BuildExitCode })
}
$findingsHash = Get-FindingsHash $findingsForHash

# Push the sync branch so the human can resume on it.
git push -u origin $Branch 2>&1 | ForEach-Object { [Console]::Error.WriteLine($_) }
if ($LASTEXITCODE -ne 0) { Write-Warning "Could not push sync branch — issue still being filed for visibility." }

$titleKindLabel = if ($Kind -eq 'build-failed') { 'build failure' } else { 'build inconclusive (timeout)' }
$title = "Upstream sync stuck after $PickedCount clean picks: $titleKindLabel ($findingsHash)"

$yamlBlock = Format-StuckYamlBlock @{
    tier          = '4'
    kind          = $Kind
    branch        = $Branch
    findings_hash = $findingsHash
    picked_count  = $PickedCount
    at            = Format-Iso8601
    host          = $env:COMPUTERNAME
}

$logSection = if ($BuildLogTail) {
    @"
<details><summary>Build log tail (last ~200 lines)</summary>

``````
$BuildLogTail
``````

</details>
"@
} else { '' }

$logPathLine = if ($BuildLogPath) {
    "**Full log:** ``$BuildLogPath`` (gitignored; on the host that ran the build)"
} else { '' }

$body = @"
> [!CAUTION]
> **Upstream sync stopped after build failed.**
>
> All $PickedCount cherry-pick(s) applied cleanly, but ``04-try-build.ps1``
> said NO before the PR could be finalized. Stop reason: **$Kind** (exit $BuildExitCode).
>
> The scheduler will keep skipping its runs until this issue is **closed**.

**Sync branch:** ``$Branch`` (push attempted — run ``git ls-remote --heads origin $Branch`` to verify it landed)
**Findings hash:** ``$findingsHash`` (re-runs of the same broken batch will match)
$logPathLine

$yamlBlock

---

$logSection
"@

$tmp = New-TemporaryFile
[System.IO.File]::WriteAllText($tmp, $body, (New-Object System.Text.UTF8Encoding($false)))

gh label create 'upstream-sync-stuck' --color 'B60205' --description 'Upstream sync blocked on a manual issue' -R microsoft/intelligent-terminal 2>$null | Out-Null

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
