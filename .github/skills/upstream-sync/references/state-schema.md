# `state.json` Schema

Path: `.github/upstream-sync/state.json` (committed on `main`).

```jsonc
{
  "version": 1,

  // Provenance for the baseline below. v1 scripts intentionally sync
  // microsoft/terminal main; these fields are not runtime configuration.
  "upstream_remote_url": "https://github.com/microsoft/terminal.git",
  "upstream_branch": "main",

  // The most recent upstream commit that has landed in this fork's main.
  // Updated only when a sync PR merges (the PR includes the state update).
  "last_synced_upstream_sha": "93bdbfaa3d62304f4b50b4ca4484da4dd08e4a1f",

  // Stuck-lock (Tier-3 — cherry-pick conflict). When non-null, the
  // scheduler exits early without touching any branch. Cleared by
  // scripts/clear-stuck.ps1 after a human merges the resolution PR.
  "stuck_on_sha": null,
  "stuck_branch": null,
  "stuck_at": null,           // ISO 8601 timestamp; null when not stuck
  "stuck_issue_url": null,    // populated by 07-open-stuck-issue.ps1

  // Stuck-lock (Tier-4 — post-pick validation failed). Set when picks
  // applied cleanly but static-scan / try-build / toolchain-preflight
  // blocked the push. Independent of stuck_on_sha but treated the same
  // way by the scheduler gate: either lock present → skip.
  // Shape (when non-null):
  //   {
  //     "kind":           "static-scan" | "build-failed" | "build-inconclusive" | "toolchain-missing",
  //     "base":           "<pre-pick origin/main SHA>",
  //     "head":           "<post-pick branch tip SHA>",
  //     "branch":         "upstream-sync/YYYY-MM-DD",
  //     "range":          ["<sha1>", "<sha2>", ...],   // picks that landed before validation said no
  //     "findings_hash":  "<16-hex sha256 prefix>",     // dedup signal across re-runs
  //     "at":             "ISO 8601",
  //     "issue_url":      "https://..."  | null         // null for toolchain-missing (infra)
  //   }
  "stuck_validation": null,

  // Last run summary (for fast inspection without grepping reports).
  "last_run": {
    "at": "2026-06-04T13:41:45+08:00",
    "host": "SH-YEELAM-D11S",
    "status": "ok",           // "ok" | "stuck" | "stuck-static-scan" | "stuck-build-failed" | "stuck-build-inconclusive" | "stuck-toolchain-missing"
    "branch": "upstream-sync/2026-06-04",
    "pr_url": "https://github.com/microsoft/intelligent-terminal/pull/999",
    "picked_count": 7,
    "dropped_pair_count": 1,
    "empty_count": 2,
    "tier0_resolutions": 1
  },

  // Rolling history — keep last 20 runs. Only `ok` and `stuck*` runs
  // write state (and therefore appear here); `no-op`, `dry-run`, and
  // `skipped-*` runs produce a local-only report and leave state.json
  // unchanged.
  "history": [
    { "at": "...", "status": "ok",      "picked_count": 7,  "pr_url": "..." },
    { "at": "...", "status": "stuck",   "stuck_on_sha": "abc...", "issue_url": "..." }
  ]
}
```

## Field rules

- **`last_synced_upstream_sha`** advances **only** when a sync PR is merged.
  The orchestrator updates this in the PR commit itself, so it lands
  atomically with the picks. Never edit by hand except via
  `clear-stuck.ps1`.
- **`stuck_on_sha`** is the Tier-3 gate. When set, `04-run-batch.ps1` exits 0
  without doing anything. This is intentional — the scheduler will keep
  ticking but will not clobber the stuck branch.
- **`stuck_validation`** is the Tier-4 gate, independent of `stuck_on_sha`.
  Either one being non-null causes the scheduler to skip. Cleared by
  `clear-stuck.ps1` — `-ResolvedThroughSha` is optional for Tier-4 (omit
  to keep the watermark and have the next run re-attempt the same range
  after the human fixed whatever validation caught).
- **`findings_hash`** in `stuck_validation` is a stable 16-hex prefix of
  the SHA-256 of the normalized findings list. If a re-run produces the
  same hash, the human knows the underlying fault is unchanged; if it
  changes, validation has moved to a new failure mode.
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
