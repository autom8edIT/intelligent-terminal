# Hook-Independent Pane ↔ Session Binding for WTA-Launched CLI Sessions

> Status: **Draft — design approved 2026-06-10.** Base branch: **`main`**.
> Intended to merge **independently and ahead of** the hookless watcher work
> (`dev/yuazha/hookless-agent-session-tracking`). Next step: implementation plan
> (writing-plans).

## Context & problem

Intelligent Terminal classifies agent sessions by `SessionOrigin`
(`agent_sessions.rs`):

- **Class A — `AgentPane`**: an ACP session WTA created for an agent pane. Bound
  via ACP `session/new`. WTA already records `(session_id, pane_session_id)` for
  these in `agent_pane_origin.rs` (v2 index).
- **Class B — `Unknown`**: an agent CLI (`copilot` / `claude` / `gemini` /
  `codex`) running in a normal shell pane. **On `main`, Class B is tracked by
  hooks**: each CLI's `wt-agent-hooks` plugin reports `agent_session_id` plus
  `WT_SESSION` (the pane GUID), so today's binding is exact — but it depends on
  hooks.

The wider "de-hook" effort removes hooks. Once they are gone, Class-B binding
must be reconstructed. The hookless branch does that for **user-typed** sessions
with a file watcher + per-CLI pid heuristics (exact for Copilot/Codex, but
ambiguous cwd-correlation for Claude/Gemini).

A subset of Class-B sessions are **not** user-typed — **WTA launches them
itself**:

- **(a)** `?<prompt>` delegation and the no-prompt background-agent action
  (`openBackgroundAgent`, Alt+Shift+B) → `wta delegate` opens a new tab whose
  command line is the agent's own interactive CLI.
- **(b)** Agent-pane recommendations that open a CLI in a new tab/panel
  (`RecommendedAction::OpenAndSend { agent }` / `Open { agent }`).
- **(c)** `/sessions` resume via the CLI's own resume flag (`ResumeCliFlag` → new
  pane running `<cli> --resume <key>`).

For these, WTA controls the launch, so it can bind deterministically **without
hooks** — by pinning the session id (`--session-id`) and reading the pane GUID
back from `create_tab`. This is the cleanest, most independent slice of the
de-hook work, so it ships **first, on `main`**.

## Goal

For WTA-launched CLI sessions whose agent supports `--session-id`
(**Copilot / Claude / Gemini**), establish the `(session id → pane GUID)`
binding **at launch, with no hooks**, by registering a *born-bound* session row
directly into the `wta-master` registry. Independently mergeable on `main`,
ahead of the hookless watcher.

## Non-goals

- **Not removing hooks.** This branch coexists with `main`'s hooks; it only adds
  a hook-independent binding *source* for the WTA-launched, pinnable subset.
- **No watcher.** No dependency on the hookless `session_watcher` (which does not
  exist on `main`).
- **No `SessionOrigin` / UI change.** Sessions stay `Unknown` (Class B); no
  change to `OriginFilter`, `/sessions` rendering or Enter/Resume routing,
  registry serialization, or `history_loader`.
- **Codex is out of scope.** It cannot pin `--session-id`, so it keeps using
  `main`'s existing Codex hook (`wt-agent-hooks/codex/.../send-event.ps1`)
  unchanged. Codex's hook-independent binding arrives later via the hookless
  watcher (Restart Manager, already exact for Codex).
- **User-typed (non-WTA-launched) sessions** are likewise untouched here — they
  stay on hooks until the hookless branch.
- Not fixing CLI resume addressability (Gemini `--resume` is index-based; Codex
  has no resume flag) — a pre-existing property of `ResumeCliFlag`.

## Scope: which launches

(a) `?<prompt>` delegation + `openBackgroundAgent` (`wta delegate`).
(b) agent-pane recommendation opens (`OpenAndSend { agent }` / `Open { agent }`).
(c) `/sessions` `ResumeCliFlag` (`<cli> --resume <key>`; id known = the resume key).

For all three, **only the pinnable agents (Copilot / Claude / Gemini)**. Codex
launches are left on the existing path.

## Key enabling facts (empirically verified 2026-06-10, Windows)

**1. `--session-id <uuid>` pins a chosen id for a NEW session:**

| CLI | pins a new id? | file lands at | id recoverable from filename? |
|---|---|---|---|
| Copilot | yes | `~/.copilot/session-state/<uuid>/events.jsonl` | yes — dir name `<uuid>` |
| Claude  | yes | `~/.claude/projects/<enc-cwd>/<uuid>.jsonl` | yes — stem `<uuid>` |
| Gemini  | yes | `~/.gemini/tmp/<cwd-slug>/chats/session-<ts>-<uuid[0:8]>.jsonl` (full uuid in content) | partial — only `uuid[0:8]` in the name |
| Codex   | no | `rollout-<ts>-<uuid>.jsonl` | n/a (cannot choose the id) |

(The Gemini filename caveat is irrelevant on this branch — we never discover the
file here; we register by the full pinned uuid. It matters only for the hookless
watcher's later reconciliation.)

**2. `create_tab` / `split_pane` return the new pane's identity:**
`TabCreationResult { UInt32 TabId; Guid SessionId; UInt64 WindowId; UInt32 Pid }`
(`TerminalProtocol.idl`). WTA receives the **pane GUID (`SessionId`) and the
pane's root Pid** synchronously at launch.

## Design

### Mechanism — born-bound registration at launch

At each in-scope launch:

```
1. id  = (a,b) generate a v4 UUID  |  (c) the resume key
2. cmd = <cli> ... <new_session_id_flag> <id>          (flag from agent_registry)
3. TabCreationResult = COM create_tab / split_pane(cmd)   -> pane GUID + Pid
4. tell wta-master:  RegisterLaunchedSession { id, cli, cwd, pane_guid }
       -> master upserts a SessionInfo with
            pane_session_id = pane_guid,
            cli_source      = cli,
            origin          = Unknown,        (Class B — unchanged)
            status          = Idle
```

No file discovery, no hooks, no PEB read — the row is **born bound** to the pane.

**Precedent**: master already records `(session_id, pane_session_id)` for Class A
agent panes (`agent_pane_origin.rs` v2), and the registry already carries the
`pane_session_id` field. This reuses that shape for the
(`Unknown`-origin) WTA-launched shell CLIs.

### `agent_registry` change

Add a capability field, mirroring the existing `resume_flag`:

```rust
/// Flag the CLI uses to pin a caller-chosen id on a NEW session,
/// e.g. "--session-id". None when unsupported.
pub new_session_id_flag: Option<&'static str>,
```

`Some("--session-id")` for copilot / claude / gemini; `None` for codex (and
unknown/custom). The launch path only does born-bound registration when this is
`Some`.

### Transport

- **(b)/(c)** originate in a helper, already connected to master → send
  `RegisterLaunchedSession` over the existing pipe.
- **(a)** originates in the short-lived `wta delegate` process → connect to the
  master pipe (`master-pipe.txt` rendezvous), send, exit. Master not running →
  no-op (no registry to populate; harmless).

### Storage / lifetime

The binding lives in master's **in-memory** registry row (`pane_session_id`).
**Ephemeral** — pane GUIDs are regenerated each WT run, so the binding is **never
persisted to disk**. Conversation history still lives in the CLI's own session
files and is reconstructed by `history_loader` as today; the pane binding is not
needed after a restart.

### Liveness — unchanged on this branch

`main`'s `SessionInfo` has no `bound_pid` field and no liveness reaper — that is a
hookless-branch concept. A born-bound row's liveness continues via the existing
source (hooks) until the hookless watcher lands. The `create_tab` Pid is captured
for diagnostics/logging only; pid-based liveness is deferred to the hookless branch.

### Coexistence with hooks (on this branch)

On `main` the pinnable CLIs' hooks still fire and report the same session id
(= the pinned uuid) and `WT_SESSION`. Because both key by the same id, a hook
event merges into the born-bound row (same pane) — no conflict. The born-bound
registration is the part that keeps binding working once hooks are removed.

### Scope boundary — binding vs activity (decided: binding-only)

This branch makes **binding** hook-independent. **Activity** (Working / Idle /
Attention) for these sessions continues via the existing source (hooks on
`main`) until the hookless watcher lands. So here a WTA-launched session is born
**bound + Live with coarse status**; fine-grained activity still arrives via
hooks. **Decided 2026-06-10: binding-only on this branch** — keeping it the
smallest, most independent slice; activity is intentionally left to
hooks/hookless.

## What is explicitly unchanged

`SessionOrigin` (stays `Unknown`), `OriginFilter`, `/sessions` UI + routing,
registry serialization, `history_loader`, user-typed Class-B binding, and Codex.

## Edge cases & failure modes

- **Master not running at (a) launch** → register is a no-op; harmless.
- **`create_tab` returns no/unexpected pane** → skip registration; the session,
  if it surfaces at all, falls to the existing path.
- **(c) resume reuses an existing row** (same id = resume key) → re-bind that
  Ended/Historical row to the new pane and flip it Live; no duplicate row.
- **Pinnable CLI with hook also present** (this branch) → hook event merges into
  the born-bound row by id; consistent (same pane).
- **Codex / user-typed** → untouched (existing hooks path).
- **Gemini filename only carries `uuid[0:8]`** → irrelevant here (we register by
  the full pinned uuid; no discovery). Becomes relevant only when the hookless
  watcher must reconcile its discovered key to this row — deferred there.

## Testing

- **Unit**: `new_session_id_flag` per CLI; the launch→command builder (contains
  `<flag> <uuid>` for pinnable agents, absent for Codex); `RegisterLaunchedSession`
  upserts a `SessionInfo` with `pane_session_id` + `origin = Unknown`.
- **Integration** (run 2026-06-10; keep as a documented manual test): each
  pinnable CLI launched headless with `--session-id <uuid>` writes its session
  under that uuid (Copilot dir / Claude stem `== uuid`; Gemini filename trailing
  group `== uuid[0:8]`, full uuid in content); `create_tab` returns a pane GUID.
- **End-to-end**: `?<prompt>` a Copilot/Claude/Gemini delegate; assert the
  `/sessions` row is born bound to the `create_tab` pane GUID with no hook
  involved in the binding, and Focus targets that pane.

## Rejected / deferred alternatives

- **New `SessionOrigin::Delegated` (three-way enum).** Rejected: conflates *who
  owns the conversation* (shell CLI vs agent pane) with *who initiated the
  launch* (user vs WTA). If a future requirement needs to surface delegate
  sessions distinctly, prefer an orthogonal provenance field
  (`initiated_by: User | Wta`). Not needed for the binding goal.
- **On-disk launch-intent index.** Rejected: the binding is ephemeral (pane
  GUIDs are per-WT-run); the in-memory registry row suffices.
- **Watcher-consults-intent join.** That is the *hookless-branch* design and
  presumes a `session_watcher` that does not exist on `main`. Not applicable
  here.
- **Codex via `--session-id`.** Unsupported by the CLI; deferred to the hookless
  watcher (Restart Manager, already exact).

## Open questions

- Should (a)'s `wta delegate` register over the master pipe, or be
  re-architected to launch *through* master (larger change, deferred)?
