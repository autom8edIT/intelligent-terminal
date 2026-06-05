# Build verification

Post-pick hard gate. Runs **after** the cherry-pick loop and **before**
the PR is opened. If the build fails, the agent either lands one focused
build-fix commit on the same branch (so it ships in the same PR) or — if
the fix is too large / scope creep — opens a Tier-4 stuck issue and exits.

The full orchestration around try-build lives in
[`SKILL.md` → Run a sync → step 7](../SKILL.md#7-build); this file is
just the contract for the `04-try-build.ps1` script and its diagnostics.

## Why this exists

A scheduler that opens PRs without proof the codebase still builds is
opening broken PRs. PR #220 was the motivating real-world failure:
every cherry-pick applied cleanly, every file looked right under
`git diff`, and the build broke because two unrelated upstream renames
landed `.resw` keys that collided with a fork-local commit. The compiler
catches that with zero false positives — `git` cannot.

Toolchain provisioning (e.g. `PlatformToolset` v143/v145) is treated as
the operator's problem, not the scheduler's: an under-provisioned host
just keeps tripping the build gate and the human notices via the open
stuck issue. We intentionally do **not** auto-bump toolset versions in
the repo on behalf of a single host.

## Try-build (`scripts/04-try-build.ps1`)

Default invocation:

```cmd
cmd.exe /c "tools\razzle.cmd && bz no_clean"
```

(`bz no_clean` = incremental Debug build of the full solution.)

Configurable via the script's `-BuildCommand` parameter. The default is
verified on the maintainer host; if the build fails, the diagnostics
include the log path and tail.

Output (returned as JSON on stdout):

| Field | Meaning |
|---|---|
| `kind` | `build-ok` / `build-failed` / `build-inconclusive` |
| `exit_code` | Process exit code (`-1` for `build-inconclusive`) |
| `duration_ms` | Wall-clock ms |
| `command` | The build command that was run |
| `log_path` | Repo-relative path to the full log (under `Generated Files/upstream-sync/<date>/build-logs/`, gitignored) |
| `log_tail` | Last ~200 lines for inline display in the stuck issue |

Timeout:

- Default 45 minutes (`-TimeoutMinutes`).
- On timeout the build is killed and classified as `build-inconclusive`.

## When the build fails

The agent's decision tree is in [`SKILL.md` step 7](../SKILL.md#7-build).
In short: try ONE focused fix commit when the cause is mechanical and
clearly caused by the pick batch; otherwise open the Tier-4 issue and
exit. Do **not** pile up multiple fix commits — the one-fix-per-PR rule
exists so the cherry-pick PR stays auditable as "upstream batch + at
most one mechanical fix".

## When the build fails for fork-unrelated reasons

If a flaky build (transient toolchain glitch, env issue, missing
PlatformToolset, ...) trips the gate:

1. The Tier-4 stuck issue gives a clear log tail.
2. A human can re-run the build locally, confirm it's transient or fix
   the host, then **close the stuck issue** to clear the lock.
3. The next scheduler tick re-attempts the same pick range from scratch.

Distinguishing transient-build from real-pick-broke-build is left to
the human reviewing the issue — too noisy to automate, and the cost
of a manual cross-check is small (~once per N runs).

## Build artifacts

`Generated Files/upstream-sync/<YYYY-MM-DD>/build-logs/` is **not**
committed — the repo root's `**/Generated Files/` gitignore rule
covers it. Build outputs under `bin/`, `obj/`, etc. follow the repo's
existing `.gitignore`.
