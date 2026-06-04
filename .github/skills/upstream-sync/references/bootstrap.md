# One-Time Bootstrap

The skill is incremental — it needs to know which upstream commit the
fork is "caught up to" before it can compute a pending range. This page
covers establishing that baseline exactly once.

## When to run

- `state.json` does not exist yet.
- `state.json` exists but `last_synced_upstream_sha` is missing/`null`.
- You manually merged some upstream commits outside this skill and want
  to fast-forward the baseline so the next sync doesn't re-pick them.

**Do NOT** re-run bootstrap on a working skill. It overwrites the
baseline and can cause the next sync to either re-pick already-synced
commits (creating empties — harmless but noisy) or to skip pending
commits (silently dropping upstream changes — bad).

## How to find the baseline SHA

The "baseline" is the most recent upstream commit whose tree is
**fully contained** in the fork's history. Pick one of:

### Method A — known last manual sync (preferred)

If you remember the last upstream sync (PR or branch), grab the upstream
SHA mentioned in that PR description / commit message:

```pwsh
git log --all --grep="upstream" --grep="microsoft/terminal" -i --oneline | head -20
```

Look for messages like `Merge upstream main @ <sha>` or
`Sync upstream up to <sha>`. That `<sha>` is your baseline.

### Method B — patch-id scan

For each recent fork commit (last ~200), get its patch-id and search
upstream for a matching patch-id:

```pwsh
git fetch upstream main
git log --format='%H' -200 | ForEach-Object {
    $pid = git show $_ | git patch-id --stable | ForEach-Object { ($_ -split ' ')[0] }
    $match = git log upstream/main --format='%H %s' | ForEach-Object {
        $usha = ($_ -split ' ',2)[0]
        $upid = git show $usha | git patch-id --stable | ForEach-Object { ($_ -split ' ')[0] }
        if ($upid -eq $pid) { $_ }
    } | Select-Object -First 1
    if ($match) { "$_ matches $match"; break }
}
```

Slow but reliable. The first match (newest fork commit with an upstream
twin) gives you the baseline.

### Method C — ask the human

If both above fail, ask the user for the baseline SHA. Do **not** guess.
A wrong baseline silently drops upstream commits.

## Initialize `state.json`

Once you have `<BASELINE_SHA>`:

```pwsh
pwsh .github/skills/upstream-sync/scripts/00-bootstrap.ps1 -BaselineSha <BASELINE_SHA>
```

This script:

1. Verifies `<BASELINE_SHA>` exists on `upstream/main`.
2. Writes a fresh `state.json` with the baseline + empty history.
3. Stages and commits `.github/upstream-sync/state.json` on a branch
   `chore/upstream-sync-bootstrap`.
4. Tells you to open a PR — do not push state changes straight to main.

## Verify

After the bootstrap PR merges, a dry run should report a non-empty
pending list (the commits upstream has made since baseline) without
actually picking anything:

```pwsh
pwsh .github/skills/upstream-sync/scripts/04-run-batch.ps1 -DryRun
```

Inspect the latest `reports/*.md` — it should look sane. **Then** enable
the scheduler.
