# Hooks-Fallback Agent Session Watcher (Hybrid Tracking)

## Abstract

Intelligent Terminal (IT) surfaces a live list of agent-CLI sessions
(Copilot / Claude / Codex / Gemini) in the `/sessions` view. On `main`, the
data that powers that list — *which* sessions exist, *which* pane each one runs
in, and *what* each one is doing (Working / Idle / Attention) — comes from the
PowerShell `wt-agent-hooks` bridge that every supported CLI loads, plus the
*born-bound* registration for sessions IT launches itself (see
[wta-launched-cli-session-binding.md](./wta-launched-cli-session-binding.md)).

That coverage has a hole: a session the user **types themselves** (`codex`,
`copilot`, … in a normal shell pane) is only tracked **if the hooks plugin is
installed** for that CLI. If the user never opted in, or uninstalled the hooks,
the session is invisible.

This spec adds a **file/process watcher as a pure fallback** that fills exactly
that hole, for all four CLIs, **without changing anything about the hook path**.
The design principle is one sentence:

> **Hooks (and born-bound) are authoritative when present; the watcher only
> ever fills the gap, and the two never double-track the same session.**

The C++ side is unchanged — this is entirely a `wta` (Rust) addition: the
watcher itself, an event-dedup gate, an in-window liveness gate, a liveness
reaper, and a codex title-extraction fix.

## Background: Class A / Class B

IT classifies every session by `SessionOrigin` (`agent_sessions.rs`):

- **Class A — `AgentPane`**: an ACP session IT created for an agent pane. Bound
  via ACP `session/new`; activity comes from the ACP stream. Never depends on
  hooks. **Out of scope here** — the watcher never tracks Class A.
- **Class B — `Unknown`**: an agent CLI running in an ordinary shell pane. On
  `main`, Class B is tracked by:
  - **born-bound** when IT launched it (`?<prompt>` delegation, recommended
    opens, `/sessions` resume) — see the companion spec; or
  - **hooks** when the user typed it *and* the CLI has `wt-agent-hooks`.

The watcher targets the **remaining** Class-B sessions: **user-typed CLIs with
no hooks installed**. It is the last slice of the "de-hook" effort, shipped as
an opt-out-free fallback rather than a hook replacement.

## Goals

- When hooks are absent, still discover user-typed Class-B sessions, bind them
  to their pane, show live activity, and reap them when the process exits.
- Never produce a duplicate, a ghost, or a wrong-pane row when hooks **are**
  present — the watcher must be a no-op for any session a hook owns.
- Never surface a session that is not actually running in **this** IT window
  (the four CLIs write their session state to per-user roots shared by VS Code,
  language servers, other terminals, and other IT windows).

## Non-goals

- Replacing or modifying the hook path, the born-bound path, or any C++ code.
- Tracking Class-A agent panes (ACP already does).
- Perfect, instant fidelity. The watcher is a *fallback*; a few seconds of lag
  or a missed transient state is acceptable. We deliberately avoid adding
  polling sweeps or CLI-specific heroics to chase the last 1%.

## Principle: two independent signals, one registry

```
                       wta-master registry  (one row per session)
                                  ▲
            ┌─────────────────────┼─────────────────────┐
            │ authoritative       │            fallback  │
   hooks / born-bound ────────────┤            ┌─────────┴── file/process watcher
   (intellterm.wta/session_hook)  │            │            (in-process, master)
            │                     │            │
     marks session in        apply_watcher_event drops the event if the
     `hook_owned` set ───────► session is already hook_owned OR is Class A
```

Both signals feed the **same reducer** and the **same registry rows**. A
master-side `hook_owned: Mutex<HashSet<SessionId>>` is the seam that keeps them
from fighting (see *Dedup* below). Everything runs always; dedup is what makes
the watcher a no-op for hook-owned sessions, so there is no "hooks mode" vs
"watcher mode" switch and no install-state probing.

## Solution design

### The watcher (discovery + activity)

`session_watcher/*` is a [`notify`](https://crates.io/crates/notify)-based
recursive file watcher over the four CLIs' session-state roots:

| CLI     | Root watched                       | Session key |
|---------|------------------------------------|-------------|
| Copilot | `~/.copilot/session-state/`        | state-dir name (uuid) |
| Claude  | `~/.claude/projects/**/*.jsonl`    | jsonl stem (uuid) |
| Codex   | `~/.codex/sessions/**/*.jsonl`     | rollout id (last 5 hyphen groups) |
| Gemini  | `~/.gemini/tmp/**/*.json`          | chat id |

It is **event-driven**: a single `for res in raw_rx` loop reacts to create /
modify events (`process_change`). There is **no periodic sweep** — an earlier
3-second hot-set sweep was removed once the watcher became a fallback, to keep
the cost proportionate. At startup, `seed_existing_progress_in` records the
files already on disk so a fresh master doesn't replay the user's entire history
as "new activity".

Each change is handed to the per-CLI classifier
(`classify_{copilot,codex,claude,gemini}.rs`), which decides whether the file
represents a real, top-level user session and what activity state it implies.
Codex subagent rollouts (`multi_agent_v1` / `spawn_agent` forks, identified by
`source.subagent` in the rollout `session_meta`) are **skipped** — they inherit
the parent's history and would otherwise appear as a duplicate row with the same
title.

### Binding & liveness (process-driven)

`proc_bind.rs` resolves the `(pane GUID, owner pid, cwd)` for a watched session,
best-effort, via Win32:

- `wt_session_for_pid` — reads the `WT_SESSION` environment variable straight
  out of a process's PEB; this **is** the pane GUID.
- `copilot_pid_from_lock` — copilot writes `inuse.<pid>.lock` in its session
  dir, giving an exact session→pid link.
- `file_owner_pid` — Restart-Manager (`RmStartSession` / `RmGetList`) reports
  which process holds a rollout file open (used for codex).
- `pid_alive`, `env_var_for_pid`, `cwd_for_pid` — supporting probes.

Binding confidence differs per CLI, and **this is the core reason hooks remain
preferred**:

- **Copilot**: lock-file → pid → `WT_SESSION` is exact.
- **Codex**: RM file-owner → pid → `WT_SESSION` is exact *while codex holds the
  rollout open*.
- **Claude / Gemini**: no lock file and no reliable open-file owner, so binding
  falls back to cwd correlation, which is ambiguous when two panes share a cwd.

A failed bind never blocks the row — it yields `pane = None` / `pid = None`. The
resolved `pid` is stored on the row as `bound_pid` (`session_registry.rs`) and
feeds the reaper.

### Dedup: how hooks and the watcher never double-track

`master/mod.rs` holds `hook_owned: Mutex<HashSet<SessionId>>`.
`handle_session_hook` inserts the session key for **every** inbound
`intellterm.wta/session_hook` event — which covers both real PowerShell hooks
**and** born-bound registration, since both arrive through that one method.

`apply_watcher_event` early-returns (drops the watcher event) when **either**:

1. `hook_owned.contains(sid)` — a hook/born-bound has claimed it; or
2. the existing registry row's `origin == AgentPane` — it's Class A (ACP).

So the moment a hook is heard for a session, the watcher goes silent for it.
There is no ordering requirement: if the watcher created the row first and a
hook arrives later, subsequent watcher events are dropped and the hook owns the
row from then on (the row identity is the same session id, so no duplicate is
produced).

### Liveness gate: scoping to this IT window

The four CLIs write their session state to **per-user** roots, so the watcher
sees *every* such session on the machine — VS Code's copilot, the
copilot-language-server, agent CLIs in other terminals, and sessions in **other
IT windows**. Surfacing those would pollute this window's list (observed: two
"Idle" copilot rows appearing before the user opened anything).

The gate (`watcher_row_allowed` + `live_it_pane_guids`) admits a watcher session
only when its **bound pane is a live pane in *this* IT instance**:

- `live_it_pane_guids` walks `list_windows → list_tabs → list_panes` over the
  COM `IProtocolServer` channel, lowercases every pane `session_id`, and caches
  the set for 2 seconds. It returns `None` when there is no WT channel (tests,
  detached master), in which case the gate is permissive.
- `watcher_row_allowed(pane, Some(set))` = `pane` is `Some` **and** in `set`.

> **Implementation note (bug class to remember):** the COM JSON returns
> `window_id` / `tab_id` as **numbers** (`"window_id": 1`), so the walk must
> match `String | Number`. An earlier `as_str()`-only extraction skipped every
> window, produced an empty live set, and silently rejected **all** watcher
> sessions. The cross-shape match is load-bearing.

The gate runs **only** when a row is being created (`None`) or revived from a
terminal state (`Historical` / `Ended`); already-live rows skip it so a chatty
session doesn't re-walk COM on every keystroke.

### The 5-second reaper

Hooks emit an explicit close event; the watcher has no such signal, so a
dedicated `tokio::time::interval(5s)` task (`reap_dead_class_b_sessions`) ends
fallback rows whose process has gone. It transitions a row to `SessionStopped`
when **all** hold:

- `origin != AgentPane` (Class B only),
- `status ∈ {Working, Idle, Attention}` (not already terminal),
- `bound_pid.is_some()` — and `bound_pid` is set **only** by the watcher bind
  path, so the reaper effectively only ever reaps watcher-tracked rows, and
- `!pid_alive(bound_pid)`.

`pid_alive` is ~13 µs for a live pid; the per-tick cost is well under 0.1 ms for
realistic row counts, so the 5 s cadence is negligible. This is a net-new task
(master has no other interval; the `/sessions` view's 5 s re-poll is
helper-side and unrelated).

### Title resolution (and the codex AGENTS.md fix)

A watcher row is created with a **synthetic** title (cwd basename, or empty),
then upgraded from the CLI's on-disk artefacts by `try_refresh_title_from_disk`
→ `lookup_title_for_session`, the **same** disk-title path the hook and
born-bound rows use:

- Copilot → `workspace.yaml` `summary:` (fallback `name:`)
- Claude / Gemini → first real user message in the jsonl
- Codex → `codex_title_from_file` (first non-synthetic user turn)

Because the path is shared, a latent codex bug affected **all three origins**,
not just the watcher: codex auto-loads `AGENTS.md` when the cwd has one and
prepends it as a synthetic user-role record (`# AGENTS.md instructions for
<dir>`) *before* the user's first prompt. The old scanner skipped only
`<environment_context>`, so it titled the session with that 69-character doc
heading instead of the prompt.

The fix is a shared `codex_user_text_is_synthetic` helper
(`history_loader.rs`) that recognises codex's injected blocks —
`<environment_context>`, `<user_instructions>`, `<subagent_notification>`,
`<turn_aborted>`, and `# AGENTS.md instructions for ` — and is used by **both**
the title scan (`codex_title_from_file`) and the phantom-session check
(`codex_session_has_real_content`, so a never-prompted codex opened in an
`AGENTS.md` repo is correctly treated as empty rather than surfaced with a doc
title).

### Components & files

| Concern | File(s) |
|---------|---------|
| Watcher loop, roots, seed | `tools/wta/src/session_watcher/mod.rs` |
| Per-CLI discovery / classify | `session_watcher/{discover,classify_copilot,classify_codex,classify_claude,classify_gemini}.rs` |
| Pane binding helper | `session_watcher/bind.rs` |
| Win32 probes (PEB, lock, RM) | `tools/wta/src/proc_bind.rs` |
| Apply / dedup / gate / reaper | `tools/wta/src/master/mod.rs` (`apply_watcher_event`, `ensure_watched_session_row`, `resolve_watched_pane_pid_cwd`, `watcher_row_allowed`, `live_it_pane_guids`, `reap_dead_class_b_sessions`, `hook_owned`) |
| Row `bound_pid` field | `tools/wta/src/session_registry.rs` |
| Codex title / subagent / phantom | `tools/wta/src/history_loader.rs` |

### Activity-state mapping

The watcher maps file evidence to the same `AgentStatus` the hook reducer uses:
a fresh/updated session is `Idle`; the per-CLI classifier promotes to `Working`
on in-progress turn markers and to `Attention` on a pending user-input prompt;
the reaper (or a hook taking over) moves it out of those states. Terminal states
are `Historical` (from the startup history scan) and `Ended` (pane/process
gone).

## What is explicitly unchanged

- The hook path, born-bound registration, and the `intellterm.wta/session_hook`
  reducer.
- All C++ (FRE "Install hooks", Settings UI, `ConptyConnection`,
  `agent_hooks_installer`, the four hook bundles).
- Class-A agent-pane tracking.
- The `/sessions` UI and its 5 s re-poll.

## Edge cases & failure modes

- **Hooks installed mid-session**: the first hook event marks the session
  `hook_owned`; the watcher row (if any) is adopted by the hook from then on,
  same session id, no duplicate.
- **Bind fails (Claude/Gemini shared cwd)**: row is created with `pane = None`;
  the liveness gate then withholds it (no live pane to match) rather than risk a
  wrong-window row. This is the conservative, accepted limitation that keeps
  hooks preferred for those two CLIs.
- **Codex holds, then releases, the rollout file**: binding is exact only while
  the file is held; if codex closes it the reaper still ends the row on process
  exit via `bound_pid`.
- **Other IT window**: that window's panes aren't in this master's
  `live_it_pane_guids`, so the gate withholds the row — each window shows only
  its own.
- **`notify` miss**: a dropped FS event means a late or missing appearance; the
  fallback nature makes this acceptable, and the startup seed bounds the blast
  radius after a restart.

## Capabilities

- **Security / Privacy**: reads only the user's own CLI session-state files and
  process metadata for the current user; no new network or cross-user access.
- **Reliability**: best-effort throughout; every probe failure degrades to "no
  row" rather than a wrong row. Dedup and the liveness gate are the two
  invariants that prevent duplicates/ghosts.
- **Performance**: event-driven (no sweep); the only timer is the 5 s reaper
  (sub-0.1 ms/tick). COM pane walks are cached 2 s and only run on
  create/revive.
- **Compatibility**: additive; with hooks installed, behaviour is identical to
  `main` (watcher events are all deduped).

## Testing

- Unit: `master::tests` (`watcher_event_*`, `watcher_row_allowed_*`,
  `live_it_pane_guids_*` — incl. numeric `window_id`/`tab_id` mock,
  `reap_*`, `session_hook_marks_*`), `history_loader::tests`
  (`codex_title_skips_injected_agents_md_instructions`,
  `codex_session_with_only_injected_context_is_phantom`, subagent filter), and
  `session_watcher` discovery/classify tests.
- Manual matrix:
  - hooks installed → row tracked by hook; master log shows watcher events
    deduped.
  - hooks uninstalled → user-typed codex tracked by the watcher with the correct
    real-prompt title (verified in an `AGENTS.md` repo); external/non-IT copilot
    sessions stay hidden; no PowerShell shell-hook events in the master log.

## Diagnostics

`wta-main_master.log` (`target: "session_watcher"`):

- `refreshed live IT pane set panes={…}` — the COM-walked live pane set.
- `watcher liveness gate decision … resolved_pane=… gated=… live_pane_count=…
  allowed=…` — per-session admit/withhold.
- `upgraded synthetic title from on-disk session artefacts … title_len=…` —
  title resolution (a 69-char codex title was the AGENTS.md regression).

## Rejected / deferred alternatives

- **Hookless for all four CLIs (the original #258 approach)** — rejected:
  Claude/Gemini binding is too ambiguous and codex's RM binding is fragile, so
  hooks must stay authoritative. This spec is the salvaged *fallback* half of
  that work.
- **Polling sweep for perfect liveness** — rejected: disproportionate for a
  fallback; event-driven + the 5 s reaper is enough.
- **Pane-is-some filter instead of the in-window gate** — rejected: machine-wide
  CLI sessions also carry `WT_SESSION`, so only membership in *this* window's
  live pane set is sufficient.

## Future considerations

- A stronger Claude/Gemini bind (e.g. a CLI-provided pid file) would let the
  watcher cover those two as confidently as Copilot/Codex.
- If a CLI gains a first-class "session ended" file marker, the reaper could
  react to it instead of polling pid liveness.
