# Static breakage scan

Post-batch hard gate. Runs after all cherry-picks succeed and **before**
push / PR creation. If any finding is `critical` or `high`, the run is
marked Tier-4 stuck (see [conflict-triage.md](./conflict-triage.md)).

## Why this exists

A clean cherry-pick is not a working cherry-pick. PR #220 demonstrated
two failure modes that git-level conflict detection misses:

1. **Duplicate `.resw` keys** — fork had appended translations for new
   keys, upstream loc-bot pick renamed old keys to the same names →
   duplicates in 26 files / 216 keys.
2. **Dropped fork-specific build suppressions** — `take-upstream`
   resolution of `src/common.build.pre.props` removed the fork-added
   `C4459` warning suppression.

Both were caught post-hoc by audit. This scan catches them pre-PR.

## What v1 checks

### 1. Duplicate `.resw` keys (baseline-diff)

For every `*.resw` file modified anywhere in the pick range, count
duplicate `<data name="...">` entries in the **pre-pick** state vs the
**post-pick** worktree. Gate on **newly-introduced** duplicates only —
preexisting duplicates are reported as `info`, not blocking.

Pre-pick state = the file content at `origin/main` (the orchestrator's
base before the picks). Post-pick = the worktree on the sync branch.

Severity:
- `critical` — any new duplicate key introduced by the pick range.
- `info`     — preexisting duplicates carried forward.

> **Implementation note (PR #220 follow-up).** Both `Get-FileTextAtRef`
> and `Get-FileTextOnDisk` MUST use absolute paths when calling
> `[System.IO.File]::ReadAllText`. .NET resolves relative paths against
> `[System.Environment]::CurrentDirectory`, NOT PowerShell's `$PWD`, so
> a relative path silently reads from a different worktree. PR #220's
> first dedup pass returned `blocking=false` while the build was still
> broken because of this exact bug — the scan was reading the dup-free
> `intelligent-terminal` main worktree instead of the dup-laden sync
> worktree. Same caveat for `git show` — capture via `cmd /c "git show
> ref:path > tmp 2>nul"` instead of PowerShell-pipeline join; otherwise
> high-Unicode (pseudo-locale) bytes get truncated by the PSObject
> formatter.

> **Operator note: manual dedup regex.** When fixing duplicates by hand
> (the orchestrator does NOT auto-dedup), the matching regex must be
> CRLF-agnostic: `(?s)([ \t]*)<data\s+name="([^"]+)"[^>]*>.*?</data>(\r?\n)?`.
> A `</data>\r?\n`-anchored regex misses inline-concatenated entries
> (`</data><data ...>...</data><data ...>...</data>`) that the loc-bot
> commits as a single appended line for fork-only keys in
> `qps-ploc/qps-ploca/qps-plocm` — exactly the second failure mode that
> blocked PR #220 build retry #4.

### 2. Fork invariants (regex must-match)

Reads `references/fork-invariants.json`. For each entry:
- Load the file at `path` from the post-pick worktree.
- Test `must_contain_regex` (case-sensitive, single-line).
- If it doesn't match → finding at the configured `severity`.

Seed: `C4459` suppression in `src/common.build.pre.props`.

Add new invariants whenever an audit finds another fork-specific item
that an upstream pick could silently strip. Keep the regex narrow.

## What v1 does NOT check (deferred to v2)

Documented here so contributors don't re-invent them and so v2 has a
clear scope:

- **Missing `#include`s** — narrow scope (newly-added quote includes
  in modified .h/.cpp/.idl, skipping `<...>`, `*.g.h`, `*.g.cpp`,
  `precomp.h`, `winrt/.*`) is technically straightforward but each
  exclusion list is a tail of false-positives. The try-build step (see
  [build-verification.md](./build-verification.md)) catches missing
  includes as compile errors with zero false positives, so v1 relies
  on the build step for this class.
- **vcxproj / vcxproj.filters drift** — same reasoning: a missing
  `ClCompile Include=` reference fails the build with a clear error.
- **MTSMSettings / GlobalAppSettings drift** — relies on the build
  step for now; a v2 addition could be a focused cross-check if a real
  miss slips past the build.

## Output

The scan emits a JSON document on stdout:

```json
{
  "findings": [
    {
      "check": "resw-duplicate-keys",
      "severity": "critical",
      "path": "src/cascadia/TerminalApp/Resources/zh-CN/Resources.resw",
      "detail": "15 newly-duplicated <data name> entries",
      "examples": ["ConfirmCloseDialog_Cancel", "..."]
    },
    {
      "check": "fork-invariant",
      "severity": "high",
      "id": "common-build-c4459-suppression",
      "path": "src/common.build.pre.props",
      "detail": "regex '\\b4459\\b' did not match"
    }
  ],
  "summary": {
    "critical": 13,
    "high": 1,
    "medium": 0,
    "low": 0,
    "info": 0
  },
  "blocking": true
}
```

`blocking` = true ⇔ any `critical` or `high` finding present.

## Extending the scan

To add a new check:

1. Add a function to `scripts/08-static-scan.ps1` that returns a list
   of finding hashtables (same shape as above).
2. Wire it into the main loop in `08-static-scan.ps1`.
3. Document the check here.
4. Add a baseline-aware test (compare pre-pick vs post-pick) **only
   when** the check would otherwise gate on preexisting issues.
5. Wire its severities into the `blocking` calculation if it should
   block the run.
