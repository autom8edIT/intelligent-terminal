# Agent Management — Design Spec

## Problem

A Windows Terminal window can host multiple panes, some of which run LLM
CLI agents (Claude Code, Copilot CLI, Gemini CLI). The user has no
single place to:

- See all agents in this window at a glance.
- Tell which agent is idle, working, blocked on a permission prompt, or
  errored out.
- Jump back to a specific agent's pane.
- See past agent sessions and resume one.

This spec defines an in-WTA TUI view that lists agent sessions (live and
historical) with status, and the data model + event plumbing that drives
it.

The WTA pane's *own* coordinator agent is **not** tracked in this list
— it is already represented by the WTA UI itself.

## Approach

Extend the existing WTA TUI with a second view (`Agents`) reachable by
Tab/F2 from the chat view. The view renders an `AgentRegistry` — an
in-process state store that aggregates events from three sources:

1. **Hooks** (primary driver of live status) — the existing
   `wt-agent-hooks` Copilot-CLI plugin (introduced in PR #11) forwards
   `PreToolUse` / `PostToolUse` / `Notification` / `Stop` /
   `SessionStart` events via `wtcli send-event`. These reach WTA through
   `wtcli listen --json`.
2. **WT protocol events** — `connection_state` (failed/closed) drives
   `Error` / `Ended` transitions when an agent's pane dies.
3. **Log scanner** (future, v2) — startup-time scan of agent CLI log
   directories rebuilds historical sessions on disk.

Sources push `SessionEvent`s into the registry; the view subscribes and
re-renders on changes. The registry is single-writer, owned by the WTA
`App`, and lives in the WTA process — no IPC needed.

This design depends on a refactor that switches the WT protocol
identity from `PaneId` (`UInt32`, per-tab, reassigned) to `SessionId`
(`Guid`, owned by `Connection`, stable). PR #11 contains that refactor;
this spec includes a migration plan to bring those changes onto the
current branch.

## Architecture

```
┌─────────────────────────────────────────────────────────────────┐
│  WTA process                                                    │
│                                                                 │
│  ┌─────────────────┐    apply(SessionEvent)                     │
│  │ Event sources   │ ─────────────────────────►┌──────────────┐ │
│  │  ├─ wtcli       │                           │AgentRegistry │ │
│  │  │   listener   │  notify(dirty=true)       │ (the model)  │ │
│  │  ├─ pane lifecyc│ ◄──────────────────────── │              │ │
│  │  └─ log scanner │                           │  HashMap<    │ │
│  │     (v2)        │                           │   AgentKey,  │ │
│  └─────────────────┘                           │   AgentSess> │ │
│                                                └──────┬───────┘ │
│                                                       │         │
│                                                 redraw│         │
│                                                       ▼         │
│  ┌──────────────────────────────────────────────────────────┐   │
│  │ App TUI (ratatui)                                        │   │
│  │   View::Chat   (existing)                                │   │
│  │   View::Agents (new) — flat list of AgentSession rows    │   │
│  └──────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────┘
```

Three-layer separation:

- **Sources** are dumb adapters: parse a raw event, build a
  `SessionEvent`, hand it to the registry. They share no state.
- **Registry** owns the entire model and is the only mutator. It
  enforces invariants (status ↔ pane binding, single active per pane,
  monotonic `last_activity_at`).
- **View** is read-only: render the registry's snapshot, dispatch user
  intents (focus, resume) back as commands to the WT protocol via
  `wtcli`.

## Data Model

### Identity

```rust
pub type AgentKey = String;  // CLI-issued conversation session id
```

`AgentKey` is just the CLI's own session id. A pane is *not* the
identity — the same pane may host successive agent sessions over its
lifetime (`claude` → exit → `claude --resume` is two distinct keys
mapping to one pane). `cli_source` lives as a field on `AgentSession`,
not in the key.

In practice all three CLIs use UUID v4 as their session id, so cross-CLI
collision is negligible. If a CLI ever ships with a non-UUID id space,
the worst case is two sessions appearing as one row — no data loss,
just a cosmetic merge. This is preferable to the lookup and rename
complexity of a composite key.

`agent_session_id` is sourced from each CLI's hook payload:

| CLI         | Source                                                     |
|-------------|------------------------------------------------------------|
| Claude Code | stdin JSON `session_id` field (all hooks)                  |
| Gemini CLI  | stdin JSON `session_id` field (all hooks)                  |
| Copilot CLI | env `COPILOT_SESSION_ID`; if absent, fall back (see below) |

#### Backwards compatibility

When a hook payload lacks `agent_session_id` (older Copilot CLI builds,
unknown CLI, partial wiring), the registry must still attribute the
event to *some* session. Fallback:

1. Look up the active session in `active_by_pane[pane_session_id]`. If
   one exists, use its `AgentKey`.
2. Otherwise, synthesize a placeholder key:
   `format!("pane:{pane_guid}")`. This keeps the entry coherent for the
   lifetime of that pane. When a real `agent_session_id` later arrives
   (via `SessionStarted`), the placeholder entry is renamed to the real
   key (single rename in `sessions`, plus update of `active_by_pane`).

### Session record

```rust
#[derive(Clone, Debug)]
pub struct AgentSession {
    pub key:               AgentKey,        // == agent session id (or "pane:<guid>" placeholder)
    pub cli_source:        CliSource,       // Claude | Copilot | Gemini | Unknown

    // pane binding — Some when alive, None when ended/historical
    pub pane_session_id:   Option<Guid>,
    pub window_id:         Option<u64>,
    pub tab_id:            Option<u32>,

    pub title:             String,         // user-readable; default = "<cli> – <cwd_basename>"
    pub cwd:               PathBuf,
    pub started_at:        SystemTime,
    pub last_activity_at:  SystemTime,

    pub status:            AgentStatus,
    pub last_error:        Option<String>,         // when status == Error
    pub current_tool:      Option<String>,         // when status == Working
    pub attention_reason:  Option<String>,         // when status == Attention

    pub log_path:          Option<PathBuf>,        // for Ended/Historical only (v2)
}

#[derive(Clone, Debug, PartialEq)]
pub enum AgentStatus {
    Idle,        // pane alive, agent waiting for prompt
    Working,     // pane alive, agent running a tool / thinking
    Attention,   // pane alive, needs user input in pane (permission, choice)
    Error,       // pane alive but agent broken (API failure, connection broken)
    Ended,       // agent stopped cleanly (Stop hook) OR pane closed
    Historical,  // reconstructed from disk log; never seen alive in this WTA process (v2)
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum CliSource { Claude, Copilot, Gemini, Unknown(String) }
```

### Registry

```rust
pub struct AgentRegistry {
    sessions:        HashMap<AgentKey, AgentSession>,
    active_by_pane:  HashMap<Guid, AgentKey>,   // O(1) lookup for pane-scoped events
    dirty:           bool,                      // main loop polls this for redraw
}

impl AgentRegistry {
    pub fn apply(&mut self, ev: SessionEvent);
    pub fn iter_sorted(&self) -> impl Iterator<Item = &AgentSession>; // by last_activity_at desc
    pub fn take_dirty(&mut self) -> bool;
}
```

### Invariants

- `pane_session_id == Some(g)` iff `status ∈ {Idle, Working, Attention, Error}`.
- `active_by_pane[g]` exists iff a session with `pane_session_id == Some(g)` exists.
- `last_activity_at` is non-decreasing per session.
- Filtering: any session whose pane is the WTA agent pane itself
  (`PaneInfo.IsAgentPane == true`) is dropped at the source layer
  before reaching the registry. Implementation: at WTA startup, query
  `wtcli list-panes` once and cache the set of Guids whose
  `IsAgentPane` is true (in practice just the WTA pane's own
  `pane_session_id`); each source filters incoming events against this
  set. Refresh the cache when a `PaneClosed` arrives for an unknown
  pane to handle pane re-spawns.

## Events and State Transitions

| `SessionEvent`                              | Source                                    | Effect                                                                                            |
|---------------------------------------------|-------------------------------------------|---------------------------------------------------------------------------------------------------|
| `SessionStarted { key, pane, cwd, title }`  | hook `agent.session.started`              | upsert; `status = Idle`; bind pane; populate metadata                                             |
| `ToolStarting { key, tool_name }`           | hook `agent.tool.starting`                | `status = Working`; `current_tool = Some(name)`                                                   |
| `ToolCompleted { key }`                     | hook `agent.tool.completed`               | if `status == Working` then `status = Idle`; `current_tool = None`                                |
| `Notification { key, message }`             | hook `agent.notification`                 | `status = Attention`; `attention_reason = Some(message)`                                          |
| `SessionStopped { key, reason }`            | hook `agent.session.stopped`              | `status = Ended`; clear pane binding; remove from `active_by_pane`                                |
| `ConnectionFailed { pane, reason }`         | WT `connection_state == "failed"`         | look up `active_by_pane[pane]`; `status = Error`; `last_error = Some(reason)`                     |
| `PaneClosed { pane }`                       | WT `connection_state == "closed"` / pane lifecycle | look up `active_by_pane[pane]`; `status = Ended`; clear pane binding                     |
| `LogScanned { entries }` (v2)               | startup disk scan                         | for each: upsert with `status = Historical`                                                       |

All events bump `last_activity_at = now()` on the affected session.

### Detection gaps (v1, documented as known limitations)

- **Copilot CLI Attention**: Copilot's hook event coverage may not
  include a clear "needs approval" signal. v1 keeps Copilot in `Working`
  during such pauses; a VT-pattern detector can be added later.
- **Agent process crash without connection death**: if the agent
  crashes but the pane (e.g. shell) survives, no event fires. Session
  stays in its last status until the user closes the pane. v1 accepts
  this; v2 may add a stale-Working timeout.

## Sources — Implementation

### Hooks (primary)

The `wt-agent-hooks` plugin from PR #11 emits events with this shape
through `wtcli send-event`:

```json
{ "event": "agent.tool.starting",
  "cli_source": "claude",
  "agent_session_id": "<sid>",
  "tool_name": "bash",
  "tool_args_summary": "ls -la" }
```

The `agent_session_id` field is **added by this spec** to PR #11's
plugin. Each hook script extracts it from the CLI's stdin (Claude,
Gemini) or env (Copilot), and includes it in the payload. When absent,
the registry falls back per the rules in *Backwards compatibility*.

The COM server (`TerminalProtocolComServer`) attaches the calling
pane's `Connection.SessionId()` (`Guid`) to every broadcast envelope —
this provides the `pane_session_id` association on the receiving side.

### WT protocol events (secondary)

Existing `wtcli listen --json` already pushes `connection_state` and
`vt_sequence` to WTA. We re-route `connection_state` into the registry
in addition to the autofix path. `vt_sequence` is unchanged (autofix
only).

### Log scanner (v2 only)

Out of scope for this spec. Notes for the future implementation:

- Claude: `~/.claude/projects/<sid>/`
- Copilot: per-platform local logs directory
- Gemini: `~/.gemini/...`

Each emits a `LogScanned` event at WTA startup, populated with `status =
Historical` and a `log_path`.

## TUI

### Layout

A single flat list, sorted by `last_activity_at` descending. Active and
history rows live in the same list — active rows naturally float to
the top because they update constantly. No section headers.

```
┌────────────────────── WTA ──────────────────────┐
│ [Chat] [Agents]   ← Tab / F2 切换               │
├─────────────────────────────────────────────────┤
│ claude — agentic-terminal     WORKING    2s    │
│ copilot — scripts             ATTENTION  12s   │
│ gemini — notes                ERROR      3m    │
│ claude — agentic-terminal                5m    │
│ copilot — scripts                        1h    │
└─────────────────────────────────────────────────┘
```

### Row anatomy

Three columns only: **Title · Status · Time**.

- **Title**: `"<cli> — <cwd_basename>"` by default; truncated with
  ellipsis if narrow.
- **Status**: rendered only for live sessions
  (`Idle / Working / Attention / Error`). For `Ended` and `Historical`
  the column is blank.
- **Time**: relative-age string from `last_activity_at` (`2s`, `12m`,
  `3h`, `2d`).

Visual differentiation between live and history is conveyed by the
status column being empty (and dim row foreground for history) — not
by section split.

### Keyboard

- `Tab` / `F2`            — switch view (Chat ↔ Agents).
- `↑` / `↓`               — move cursor.
- `Enter` on **live row** — `FocusPane(pane_session_id)` via wtcli.
- `Enter` on **history row** — open new pane via `SplitPane(...)` with
  the CLI-specific resume command:
  - Claude:  `claude --resume <agent_session_id>`
  - Copilot: `copilot --resume <agent_session_id>` *(verify against
    actual Copilot CLI flag at impl time)*
  - Gemini:  `gemini --resume <agent_session_id>`
- `Delete` on history row — remove from registry (in-memory only; v2
  will offer disk deletion).

### Sort order

Single key: `last_activity_at` desc. Stable on equal timestamps.

## PR #11 Migration Plan

PR #11 (`pane-identity-and-agent-hooks`) bundles two logical changes:
the SessionId refactor and the `wt-agent-hooks` plugin. Its base
(`f53ce6882`) is roughly a month behind the current branch (PR #12
merged, `FocusPane`, autofix rework, XAML titlebar). A normal merge
explodes in conflicts.

### Strategy: two squashed cherry-picks, in order

**Batch 1 — SessionId refactor** (commits
`adac19617..8a0a0ab87`, ~9 commits squashed into one):

```bash
git checkout -b dev/yuazha/agent-management dev/yuazha/session
git cherry-pick -n adac19617^..8a0a0ab87
# resolve conflicts (see below), build, test, commit
```

Expected conflict points and resolutions:

- `src/cascadia/TerminalProtocol/TerminalProtocol.idl` — current branch
  added `Cwd`, `HasMarks`, and `FocusPane(UInt32 paneId)` after PR #11's
  base. Keep these fields, but apply PR #11's `UInt32 PaneId` →
  `Guid SessionId` rename to *all* methods including the new `FocusPane`.
  Bump protocol version `1.1` → `1.2` (PR #11's bump is preserved).
- `src/cascadia/WindowsTerminal/TerminalProtocolComServer.{h,cpp}` —
  same: rename across all method signatures, including `FocusPane`.
- `src/cascadia/TerminalApp/TerminalPage.Protocol.{h,cpp}` — replace
  `FindPaneById(uint32_t)` lookups with
  `FindPaneBySessionId(winrt::guid)`. The new autofix wiring
  (`_ensurePageEventsRegistered`, `ProtocolVtSequenceReceived`) emits
  events keyed by pane Guid (not pane `_id`).
- `src/cascadia/TerminalApp/TerminalPage.{h,cpp}` — header signatures
  updated; merge with the listener-registration code added on the
  current branch.
- `wta/src/app.rs` — rename `pane_id: String` → `session_id: String`
  (Guid text form). Update all `classify_wt_event` tests to use Guid
  strings instead of decimal pane numbers.
- `wtcli` — flags `--pane-id` → `--session-id`. Keep `--pane-id` as a
  deprecated alias accepting Guid text for one release.

Build, run autofix tests, verify `wtcli list-panes` returns Guids.

**Batch 2 — Hooks plugin + `agent_session_id` enrichment**
(commits `281495e1c..7392e311d` squashed):

```bash
git cherry-pick -n 281495e1c^..7392e311d
# resolve conflicts in app.rs (agent_event handling already touched
# by Batch 1's tests), then add the agent_session_id payload patch
```

Then apply this spec's hook-script enrichment patch on top:

- `pre-tool-use`, `post-tool-use`, `session-start`, `session-stop`,
  `notification` — each extracts `agent_session_id` from stdin
  (Claude/Gemini) or env (Copilot) and includes it in the
  `wtcli send-event` payload.
- Bump protocol version `1.2` → `1.3` to advertise the new field.

Build, run an end-to-end smoke test: open a pane, `claude` →
`tool.starting` event observed in WTA with `agent_session_id`.

### Why squash, not preserve PR #11's commit history

PR #11's intermediate commits don't compile against the current branch
in isolation (`FocusPane`, autofix listener, etc. weren't there).
Replaying them one-by-one would mean each commit fails to build until
the last. A single squashed commit per batch is honest about that.

## Milestones

| M  | Scope                                                | Done when                                                                                     |
|----|------------------------------------------------------|-----------------------------------------------------------------------------------------------|
| M1 | Batch 1 cherry-pick: SessionId refactor              | Solution builds; autofix tests green; `wtcli list-panes` shows Guids                          |
| M2 | Batch 2 cherry-pick + `agent_session_id` enrichment  | Hook fires on each CLI; WTA sees `agent_event` with `agent_session_id` populated              |
| M3 | `AgentRegistry` model + source wiring (no UI)        | Unit tests on event sequences; debug log shows correct status transitions per CLI             |
| M4 | `AgentListView` TUI + Focus / Resume                 | Tab switches view; `Enter` focuses live pane; `Enter` on history spawns a resume pane         |

Each milestone is independently committable and verifiable.

## Out of scope (v2+)

- Disk log scanner / `Historical` status population.
- VT-pattern fallback detection for Attention (Copilot CLI gap).
- Stale-`Working` timeout heuristic.
- Per-session detail view (transcript scroll, tool-call timeline).
- Cross-window aggregation (this spec is single-window only).
- Persistence of registry state across WTA restarts.
