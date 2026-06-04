# `state.json` Schema

Path: `.github/upstream-sync/state.json` (committed on `main`).

```jsonc
{
  "version": 1,
  "upstream_remote_url": "https://github.com/microsoft/terminal.git",
  "upstream_branch": "main",

  // The most recent upstream commit that has landed in this fork's main.
  // Updated only when a sync PR merges (the PR includes the state update).
  "last_synced_upstream_sha": "93bdbfaa3d62304f4b50b4ca4484da4dd08e4a1f",

  // Stuck-lock. When non-null, the scheduler exits early without touching
  // any branch. Cleared by scripts/clear-stuck.ps1 after a human merges
  // the resolution PR.
  "stuck_on_sha": null,
  "stuck_branch": null,
  "stuck_at": null,           // ISO 8601 timestamp; null when not stuck
  "stuck_issue_url": null,    // populated by 07-open-stuck-issue.ps1

  // Last run summary (for fast inspection without grepping reports).
  "last_run": {
    "at": "2026-06-04T13:41:45+08:00",
    "host": "SH-YEELAM-D11S",
    "status": "ok",           // "ok" | "no-op" | "stuck" | "skipped-locked"
    "branch": "upstream-sync/2026-06-04",
    "pr_url": "https://github.com/microsoft/intelligent-terminal/pull/999",
    "picked_count": 7,
    "dropped_pair_count": 1,
    "empty_count": 2,
    "tier0_resolutions": 1
  },

  // Rolling history — keep last 20 runs.
  "history": [
    { "at": "...", "status": "ok",      "picked_count": 7,  "pr_url": "..." },
    { "at": "...", "status": "no-op",   "picked_count": 0 },
    { "at": "...", "status": "stuck",   "stuck_on_sha": "abc...", "issue_url": "..." }
  ]
}
```

## Field rules

- **`last_synced_upstream_sha`** advances **only** when a sync PR is merged.
  The orchestrator updates this in the PR commit itself, so it lands
  atomically with the picks. Never edit by hand except via
  `clear-stuck.ps1`.
- **`stuck_on_sha`** is the gate. When set, `04-run-batch.ps1` exits 0
  without doing anything. This is intentional — the scheduler will keep
  ticking but will not clobber the stuck branch.
- **`stuck_branch`** must still exist on `origin` until the human merges
  it; `clear-stuck.ps1` does not delete it (the PR merge does).
- **`history`** is for the human reading state.json directly. The reports
  in `reports/` are the source of truth.

## Concurrency

The scheduler should run on a single host. If multiple hosts run
concurrently, the second one's `git push -u origin upstream-sync/<date>`
will collide on the same-day branch name — `git push` will reject with
non-fast-forward and `04-run-batch.ps1` will exit 20 (hard failure).
This is acceptable: the loser's report is still written locally for
inspection, and no state on `main` has been updated.

If you genuinely need multi-host scheduling, add a per-host suffix to
the branch name and a state-file mutex via `gh api repos/.../contents/...`
GraphQL check-and-set — out of scope for v1.
