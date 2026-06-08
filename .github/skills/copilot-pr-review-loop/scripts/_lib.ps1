# Shared helpers for copilot-pr-review-loop scripts.
# Dot-source with: `. "$PSScriptRoot/_lib.ps1"`
#
# Dot-sourcing runs the prerequisite check below; if `gh` is missing or
# unauthenticated the script halts BEFORE doing any work, with a single
# actionable error message the calling agent can pattern-match on.

# Prerequisite check: gh CLI installed AND authenticated.
# Fails fast with install/login instructions. Runs once per PowerShell
# session (idempotent — re-dot-sourcing is a no-op after success).
function Assert-GhReady {
    if ($script:_GhReady) { return }

    # 1. Installed?
    $cmd = Get-Command gh -ErrorAction SilentlyContinue
    if (-not $cmd) {
        throw @'
copilot-pr-review-loop: prerequisite missing — `gh` CLI is not on PATH.

Install (one of):
  - winget install --id GitHub.cli           (Windows)
  - brew install gh                          (macOS)
  - sudo apt install gh                      (Debian/Ubuntu — see https://cli.github.com for other distros)
  - https://cli.github.com/                  (universal installer + download)

Then `gh auth login` and re-run this command.
'@
    }

    # 2. Authenticated? `gh auth status` exits non-zero when no account
    # is logged in. We can't call Invoke-Gh (defined below this function),
    # so use ProcessStartInfo directly — same .NET path, no PS `2>`
    # redirect (which inherits the caller's WhatIf and would print
    # spurious "Performing the operation Output to File" noise on -WhatIf
    # runs of consuming scripts).
    $psi = [System.Diagnostics.ProcessStartInfo]::new('gh')
    $null = $psi.ArgumentList.Add('auth')
    $null = $psi.ArgumentList.Add('status')
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $proc = [System.Diagnostics.Process]::Start($psi)
    try {
        $errTask = $proc.StandardError.ReadToEndAsync()
        $outTask = $proc.StandardOutput.ReadToEndAsync()
        $proc.WaitForExit()
        # Await both async reads before disposing so neither leaks an
        # unobserved Task / faulted continuation, and so stderr is fully
        # drained when we read it on the failure path. Stdout content is
        # discarded but the task must still be awaited.
        $null = $outTask.GetAwaiter().GetResult()
        $err = $errTask.GetAwaiter().GetResult()
        if ($proc.ExitCode -ne 0) {
            throw @"
copilot-pr-review-loop: prerequisite missing — ``gh`` CLI is not authenticated.

Run:
  gh auth login

Then re-run this command. (``gh auth status`` reported:
  $($err.Trim()))
"@
        }
    } finally {
        $proc.Dispose()
    }

    $script:_GhReady = $true
}

# Single-invocation gh wrapper. Captures stdout + stderr separately
# via .NET ProcessStartInfo (with async stream reads to avoid the
# deadlock risk on chatty stderr) and returns ExitCode/Stdout/Stderr.
# Going through .NET — not PowerShell's `2>` redirect — sidesteps the
# `Out-File`/-WhatIf inheritance that otherwise prints
# `What if: Performing the operation "Output to File"` noise when the
# calling script is invoked with -WhatIf.
function Invoke-Gh {
    param([Parameter(Mandatory)][string[]]$GhArgs)
    $psi = [System.Diagnostics.ProcessStartInfo]::new('gh')
    foreach ($a in $GhArgs) { $null = $psi.ArgumentList.Add($a) }
    $psi.RedirectStandardOutput = $true
    $psi.RedirectStandardError = $true
    $psi.UseShellExecute = $false
    $proc = [System.Diagnostics.Process]::Start($psi)
    try {
        $outTask = $proc.StandardOutput.ReadToEndAsync()
        $errTask = $proc.StandardError.ReadToEndAsync()
        $proc.WaitForExit()
        [pscustomobject]@{
            ExitCode = $proc.ExitCode
            Stdout   = $outTask.GetAwaiter().GetResult()
            Stderr   = $errTask.GetAwaiter().GetResult()
        }
    } finally {
        $proc.Dispose()
    }
}

# Wrapper around Invoke-Gh for `gh api graphql` that throws on either
# non-zero exit OR a GraphQL `errors` array in the response body.
function Invoke-GhGraphQL {
    param(
        [Parameter(Mandatory)][string[]]$GhArgs,
        [Parameter(Mandatory)][string]$Context
    )
    $r = Invoke-Gh -GhArgs (@('api','graphql') + $GhArgs)
    if ($r.ExitCode -ne 0) {
        throw "gh api graphql failed (exit $($r.ExitCode)) [$Context]: $($r.Stderr)"
    }
    $data = $r.Stdout | ConvertFrom-Json
    if ($data.errors) {
        $msgs = ($data.errors | ForEach-Object { $_.message }) -join '; '
        throw "GraphQL errors [$Context]: $msgs"
    }
    $data
}

# Auto-resolve owner/repo from gh's local context when caller didn't pass them.
function Resolve-RepoCoords {
    param([string]$Owner, [string]$Repo)
    if ($Owner -and $Repo) { return @{ Owner = $Owner; Repo = $Repo } }
    $r = Invoke-Gh -GhArgs @('repo','view','--json','owner,name')
    if ($r.ExitCode -ne 0) {
        throw "gh repo view failed (exit $($r.ExitCode)): $($r.Stderr). Pass -Owner and -Repo explicitly, or run from inside a gh-detected repo."
    }
    $info = $r.Stdout | ConvertFrom-Json
    @{
        Owner = if ($Owner) { $Owner } else { $info.owner.login }
        Repo  = if ($Repo)  { $Repo }  else { $info.name }
    }
}

# Run the prerequisite check as a side-effect of dot-sourcing.
Assert-GhReady
