<#
.SYNOPSIS
  Static breakage scan. Runs AFTER all cherry-picks succeed and BEFORE
  push / PR creation. Catches "clean cherry-pick but broken content"
  failures that git-level conflict detection misses (PR #220 audit).

.DESCRIPTION
  Two v1 checks (see references/static-scan.md for v2 deferred items):

    1. Duplicate <data name=...> entries in *.resw files (baseline-diff —
       only gates on NEW duplicates introduced by the pick range).

    2. Fork invariants — regex patterns from references/fork-invariants.json
       that must still match in the post-pick worktree.

.PARAMETER BaseSha
  Pre-pick base commit (usually origin/main at orchestrator start). Used
  to compute baseline-diff for the resw check. Required.

.PARAMETER HeadRef
  Post-pick worktree ref (default: HEAD).

.OUTPUTS
  Emits a single JSON document on stdout. Exit code:
    0 = scan ran cleanly (findings may still be present — inspect JSON)
    20 = scan itself errored out (broken script, missing files, etc.)
#>
[CmdletBinding()]
param(
    [Parameter(Mandatory)] [string] $BaseSha,
    [string] $HeadRef = 'HEAD'
)

. "$PSScriptRoot/Common.ps1"

function Get-ReswDuplicateNames {
    param([string] $Text)
    if (-not $Text) { return @() }
    $names = [System.Collections.Generic.List[string]]::new()
    $re = [regex]'<data\s+name="([^"]+)"'
    foreach ($m in $re.Matches($Text)) { $names.Add($m.Groups[1].Value) }
    $dups = $names | Group-Object | Where-Object { $_.Count -gt 1 } | ForEach-Object { $_.Name }
    return @($dups)
}

function Get-ChangedReswFiles {
    param([string] $Base, [string] $Head)
    $out = git diff --name-only --diff-filter=ACMR "$Base..$Head" 2>$null
    if ($LASTEXITCODE -ne 0) { throw "git diff failed computing changed resw files." }
    return @($out | Where-Object { $_ -like '*.resw' })
}

function Get-FileTextAtRef {
    param([string] $Ref, [string] $Path)
    $text = git show "${Ref}:$Path" 2>$null
    if ($LASTEXITCODE -ne 0) { return $null }   # file didn't exist at that ref
    return ($text -join "`n")
}

function Get-FileTextOnDisk {
    param([string] $Path)
    if (-not (Test-Path -LiteralPath $Path)) { return $null }
    return [System.IO.File]::ReadAllText($Path)
}

function Scan-ReswDuplicates {
    param([string] $Base, [string] $Head)
    $findings = @()
    foreach ($f in (Get-ChangedReswFiles -Base $Base -Head $Head)) {
        $baseText = Get-FileTextAtRef -Ref $Base -Path $f
        $headText = if ($Head -eq 'HEAD') { Get-FileTextOnDisk -Path $f } else { Get-FileTextAtRef -Ref $Head -Path $f }
        $baseDups = @(Get-ReswDuplicateNames -Text $baseText)
        $headDups = @(Get-ReswDuplicateNames -Text $headText)
        $newDups  = @($headDups | Where-Object { $baseDups -notcontains $_ })
        $oldStill = @($headDups | Where-Object { $baseDups -contains $_ })
        if ($newDups.Count -gt 0) {
            $findings += [ordered] @{
                check     = 'resw-duplicate-keys'
                severity  = 'critical'
                path      = $f
                detail    = "$($newDups.Count) newly-duplicated <data name> entries (was $($baseDups.Count) at base)"
                examples  = @($newDups | Select-Object -First 5)
            }
        }
        if ($oldStill.Count -gt 0) {
            $findings += [ordered] @{
                check     = 'resw-duplicate-keys'
                severity  = 'info'
                path      = $f
                detail    = "$($oldStill.Count) duplicate <data name> entries also present pre-pick (not blocking)"
                examples  = @($oldStill | Select-Object -First 5)
            }
        }
    }
    return ,$findings
}

function Scan-ForkInvariants {
    $findings = @()
    $invPath = Join-Path (Split-Path -Parent $PSScriptRoot) 'references/fork-invariants.json'
    if (-not (Test-Path -LiteralPath $invPath)) {
        return ,@([ordered] @{
            check    = 'fork-invariants'
            severity = 'medium'
            path     = $invPath
            detail   = 'fork-invariants.json missing — cannot check fork-protected items'
        })
    }
    $doc = Get-Content -Raw -LiteralPath $invPath | ConvertFrom-Json
    foreach ($inv in @($doc.invariants)) {
        $absPath = Join-Path (Get-RepoRoot) $inv.path
        if (-not (Test-Path -LiteralPath $absPath)) {
            $findings += [ordered] @{
                check    = 'fork-invariant'
                severity = $inv.severity
                id       = $inv.id
                path     = $inv.path
                detail   = "protected file does not exist in worktree"
                reason   = $inv.reason
            }
            continue
        }
        $text = [System.IO.File]::ReadAllText($absPath)
        $re = [regex]::new($inv.must_contain_regex)
        if (-not $re.IsMatch($text)) {
            $findings += [ordered] @{
                check    = 'fork-invariant'
                severity = $inv.severity
                id       = $inv.id
                path     = $inv.path
                detail   = "regex '$($inv.must_contain_regex)' did not match in post-pick file"
                reason   = $inv.reason
            }
        }
    }
    return ,$findings
}

try {
    $findings = @()
    $findings += Scan-ReswDuplicates -Base $BaseSha -Head $HeadRef
    $findings += Scan-ForkInvariants

    $summary = [ordered] @{
        critical = @($findings | Where-Object { $_.severity -eq 'critical' }).Count
        high     = @($findings | Where-Object { $_.severity -eq 'high'     }).Count
        medium   = @($findings | Where-Object { $_.severity -eq 'medium'   }).Count
        low      = @($findings | Where-Object { $_.severity -eq 'low'      }).Count
        info     = @($findings | Where-Object { $_.severity -eq 'info'     }).Count
    }
    $blocking = ($summary.critical + $summary.high) -gt 0

    $doc = [ordered] @{
        base     = $BaseSha
        head     = $HeadRef
        findings = @($findings)
        summary  = $summary
        blocking = $blocking
    }
    $doc | ConvertTo-Json -Depth 8
    exit 0
}
catch {
    Write-Error $_.Exception.Message
    Write-Error $_.ScriptStackTrace
    exit 20
}
