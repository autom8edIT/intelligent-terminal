# Common.ps1 — shared helpers for upstream-sync scripts.
# Dot-source from each script:  . "$PSScriptRoot/Common.ps1"
#
# Only contains helpers used by 2+ scripts. Single-use helpers live inline
# in the script that uses them.
#
# The skill keeps no `state.json`. Watermark is the most recent
# `(cherry picked from commit <sha>)` trailer on origin/main (read by
# 02-compute-pending.ps1). Stuck-lock is any OPEN gh issue with the
# `upstream-sync-stuck` label (agent queries `gh issue list` directly).

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$script:WtaStateFence = '# wta-state'

function Format-StuckYamlBlock {
    # Fenced ```yaml ... # wta-state ... ``` block embedded in stuck-issue
    # bodies. Values are single-quoted with `'` -> `''` escaping so colons,
    # newlines, leading dashes round-trip through the issue body. Used by
    # 06-open-stuck-issue.ps1 and 06b-open-build-stuck-issue.ps1.
    param([Parameter(Mandatory)] [hashtable] $Fields)
    $lines = @('```yaml', $script:WtaStateFence)
    foreach ($k in $Fields.Keys) {
        $raw = "$($Fields[$k])"
        $folded = $raw -replace '\r?\n', ' '
        $escaped = $folded -replace "'", "''"
        $lines += ("{0}: '{1}'" -f $k, $escaped)
    }
    $lines += '```'
    return ($lines -join "`n")
}

function Format-Iso8601 {
    # Used by 06 + 06b for the `at` field of the stuck-issue YAML block.
    param([DateTime] $When = (Get-Date))
    return $When.ToString('yyyy-MM-ddTHH:mm:sszzz')
}
