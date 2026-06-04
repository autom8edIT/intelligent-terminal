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
  Emits a single JSON document on stdout.

  Error model:
    Throws on wrapper error (broken script, missing files, etc.). The
    orchestrator (`04-run-batch.ps1`) catches and routes through its
    own exit-code mapping (0 ok / 10 stuck / 20 error). Run standalone,
    an uncaught throw exits with PowerShell's default code (1) plus a
    stack trace. `exit 20` is intentionally NOT used here because this
    script is invoked via `&` from the orchestrator — `exit` in that
    context would terminate the orchestrator mid-pipeline.
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
    # Capture via a temp file to avoid PowerShell mangling binary-ish output
    # (UTF-8 BOM, mixed CRLF/LF, high-Unicode pseudo-locale glyphs) when the
    # subprocess's stdout is bound to a PSObject pipeline.
    $tmp = [System.IO.Path]::GetTempFileName()
    try {
        & cmd /c "git show ""${Ref}:$Path"" > ""$tmp"" 2>nul"
        if ($LASTEXITCODE -ne 0) { return $null }
        return [System.IO.File]::ReadAllText($tmp)
    } finally {
        Remove-Item -LiteralPath $tmp -ErrorAction SilentlyContinue
    }
}

function Get-FileTextOnDisk {
    param([string] $Path)
    # IMPORTANT: [System.IO.File]::* APIs resolve relative paths against
    # [Environment]::CurrentDirectory, NOT PowerShell's $PWD. Any relative
    # path passed in here would silently read from the wrong worktree (the
    # PR #220 audit miss). Resolve to absolute against the repo root.
    if ([System.IO.Path]::IsPathRooted($Path)) {
        $abs = $Path
    } else {
        $abs = Join-Path (Get-RepoRoot) $Path
    }
    if (-not (Test-Path -LiteralPath $abs)) { return $null }
    return [System.IO.File]::ReadAllText($abs)
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
}
catch {
    Write-Error $_.Exception.Message
    Write-Error $_.ScriptStackTrace
    throw
}
