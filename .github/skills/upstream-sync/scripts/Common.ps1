# Common.ps1 — shared helpers for upstream-sync scripts.
# Dot-source from each script:  . "$PSScriptRoot/Common.ps1"

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-RepoRoot {
    $r = git rev-parse --show-toplevel 2>$null
    if ($LASTEXITCODE -ne 0) { throw "Not inside a git repo." }
    return $r.Trim()
}

function Get-StateDir {
    Join-Path (Get-RepoRoot) '.github/upstream-sync'
}

function Get-StatePath {
    Join-Path (Get-StateDir) 'state.json'
}

function Get-ReportsDir {
    $d = Join-Path (Get-StateDir) 'reports'
    if (-not (Test-Path $d)) { New-Item -ItemType Directory -Path $d | Out-Null }
    return $d
}

function Read-State {
    $p = Get-StatePath
    if (-not (Test-Path $p)) {
        throw "state.json not found at $p. Run scripts/00-bootstrap.ps1 first — see references/bootstrap.md."
    }
    return Get-Content -Raw -LiteralPath $p | ConvertFrom-Json -AsHashtable
}

function Write-State {
    param([Parameter(Mandatory)] $State)
    $p = Get-StatePath
    $json = $State | ConvertTo-Json -Depth 12
    # Use UTF-8 *without* BOM to match git's default text handling on this repo.
    [System.IO.File]::WriteAllText($p, $json, (New-Object System.Text.UTF8Encoding($false)))
}

function Ensure-UpstreamRemote {
    param(
        [string] $Name = 'upstream',
        [string] $Url  = 'https://github.com/microsoft/terminal.git'
    )
    $existing = git remote get-url $Name 2>$null
    if ($LASTEXITCODE -ne 0) {
        git remote add $Name $Url | Out-Null
        if ($LASTEXITCODE -ne 0) { throw "Failed to add remote $Name." }
    } elseif ($existing.Trim() -ne $Url) {
        Write-Warning "Remote '$Name' exists but points at '$existing' (expected '$Url'). Leaving as-is."
    }
}

function Assert-CleanWorktree {
    $dirty = git status --porcelain
    if ($LASTEXITCODE -ne 0) { throw "git status failed." }
    if ($dirty) {
        throw "Working tree is not clean:`n$dirty`nCommit or stash first."
    }
}

function Get-GhUserLogin {
    $login = gh api user --jq '.login' 2>$null
    if ($LASTEXITCODE -ne 0 -or -not $login) { throw "gh CLI is not authenticated. Run 'gh auth login'." }
    return $login.Trim()
}

function Format-Iso8601 {
    param([DateTime] $When = (Get-Date))
    return $When.ToString('yyyy-MM-ddTHH:mm:sszzz')
}

function Format-ReportFilename {
    param([DateTime] $When = (Get-Date), [string] $Suffix = '')
    $stamp = $When.ToString('yyyy-MM-ddTHHmm')
    if ($Suffix) { return "$stamp-$Suffix.md" }
    return "$stamp.md"
}

function New-RunContext {
    [pscustomobject] @{
        StartedAt   = Get-Date
        Host        = $env:COMPUTERNAME
        Branch      = "upstream-sync/$((Get-Date).ToString('yyyy-MM-dd'))"
        Picked      = @()
        DroppedPairs= @()
        SkippedEmpty= @()
        Tier0       = @()
        Tier2       = @()
        StuckSha    = $null
        StuckPaths  = @()
        Status      = 'unknown'
        ReportPath  = $null
        PrUrl       = $null
        IssueUrl    = $null
    }
}
