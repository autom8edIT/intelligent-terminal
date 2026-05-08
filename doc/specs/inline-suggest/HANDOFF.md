# Inline Agent — Cross-Machine Handoff

**Branch:** `dev/vanzue/inline-agent`
**Status as of last commit (63bb69cb2, 2026-05-08):** Phase 0 complete (4 features committed, all reviewed at 99% senior-developer-no-concern bar). Local build blocked by missing Windows SDK 10.0.22621.0 and .NET SDK on the originating machine.

---

## How to resume on a new machine

1. `git clone` (or `git pull`) the repo and check out `dev/vanzue/inline-agent`.
2. Read these four plan documents (in order). They are the canonical source of truth for the design:
   - `doc/specs/inline-suggest/01-plan.md` — master plan, phases, success criteria, kill criteria, ownership table, manual validation matrix
   - `doc/specs/inline-suggest/02-architecture.md` — components, IDL, threading, EditLineState API, PreviewSource priority, accept-path, resilience contract, instrumentation plan
   - `doc/specs/inline-suggest/03-ux.md` — interaction model, defaults, edge cases, settings, telemetry/privacy split, shell integration prerequisite, accessibility
   - `doc/specs/inline-suggest/04-risks.md` — 27 risks with mitigations + top-5 list
3. Read this `HANDOFF.md` for status + open follow-ups + Phase 0.5 entry point.
4. To bring the next Copilot CLI session up to speed, paste the `## Prompt to bootstrap the next agent session` section below into the new CLI.

---

## What's done — Phase 0 (4 commits on this branch)

| Feat | Commit | Files | Lines | Description |
|---|---|---|---|---|
| C | `c7212cb66` | `ControlCore.{cpp,h,idl}` | +17 | `IsPasteInProgress()` atomic flag, set in `PasteText` body with release/acquire memory ordering. |
| A | `b260c7e37` | `textBuffer.{hpp,cpp}`, `Terminal.{hpp,cpp}` | +152 | `TextBuffer::EditLineSnapshot` struct + `CurrentEditLineSnapshot()` (cursor-clipped via `_commandForRow`), `Terminal::CurrentEditLineSnapshot()` passthrough authoritatively setting `inAltBuffer`. |
| B | `4fc78ed7e` | `ControlCore.{cpp,h,idl}` | +91 | IDL `EditLineState` struct, `ControlCore::GetEditLineState() const`, `IsInAlternateScreenBuffer()`, `EditLineStateChanged` event with mutex-protected last-snapshot coalescing fired from `_connectionOutputHandler`. |
| D | `63bb69cb2` | `EditLineStateTests.cpp` (new), `Control.UnitTests.vcxproj` | +248 | 10 invariant unit tests using existing `ControlCoreTests.cpp` MockControlSettings/MockConnection fixture pattern. |

Each feature went through **coder → reviewer → fix → re-review** loops and was approved before commit.

### Public API now available on `ControlCore`

```idl
struct EditLineState
{
    String CursorPrefix;        // text from prompt mark up to cursor (clipAtCursor=true)
    Boolean CursorAtEnd;
    Boolean HasPromptMark;
    Boolean CommandRunning;
    Boolean InAltBuffer;
};

EditLineState GetEditLineState();   // const
Boolean IsInAlternateScreenBuffer();
Boolean IsPasteInProgress();
event Windows.Foundation.TypedEventHandler<Object, Object> EditLineStateChanged;
```

---

## How to validate Phase 0 locally

The originating machine could not finish a build because it was missing the Windows 11 SDK and .NET SDK (build errors `MSB8036` for SDK 10.0.17134.0+ and `MSB4236` for `Microsoft.NET.Sdk`). Zero code-level errors in the build log.

On the new machine:

1. Visual Studio Installer → make sure **Visual Studio 2022 Build Tools** (or VS 2022 17.10+) has:
   - ✅ Desktop development with C++
   - ✅ Universal Windows Platform development
   - ✅ Windows 11 SDK (10.0.22621.0)
   - ✅ .NET SDK (latest)
2. Build: `cmd /c "tools\razzle.cmd && bz no_clean"`
3. Run new tests: `runut Control.UnitTests.dll /name:*EditLineStateTests*` — expects 10/10 pass.

---

## Phase 0 follow-up items (carry into next phase)

These were captured during reviews. They do not block Phase 0 sign-off but must be addressed before they cascade.

1. **PowerShell shell-integration script does not emit `OSC 133;C`** (`shell-integration 1.ps1:44-66` and embedded copy in `AppActionHandlers.cpp:1819-1887` only emit `D`, `A`, `9;9`, `B`). The current `commandRunning` derivation depends on `mark.HasCommand()` which depends on `commandEnd` set by `OSC 133;C`. Test 6 emits `;C` synthetically, but production users will see `commandRunning` always-false. **Pick one in Phase 0.5:**
   - Update the script to emit `;C` at command execution (1-line addition before `Invoke-Expression`/cmdlet call), OR
   - Refine `commandRunning` derivation to NOT require `commandEnd` (use mark transitions or cursor below `;B` row instead).

2. **Lock-semantics inconsistency**: `Terminal::CurrentCommand` does NOT acquire `LockForReading` while our new `Terminal::CurrentEditLineSnapshot` does. This is an existing-code problem flagged for a separate cleanup commit; out of scope for our feature.

3. **Real-pwsh PSReadLine integration test** for `cursorPrefix` exclusion is deferred to manual UX validation in Phase 0.5 — synthetic test (`GhostTextNotIncluded`) covers the API surface but does not exercise live PSReadLine prediction rendering.

4. **Latency baseline measurement** is required before Phase 1 entry (per Kill Criteria in `01-plan.md`). Need to instrument keystroke→PTY path and measure feature-OFF baseline + feature-ON-no-suggestion overhead.

---

## What's NOT done — next phases

| Phase | Output | Days | Gate |
|---|---|---|---|
| **0.5** | PreviewText suitability spike (acrylic/retro/SW renderer/fullscreen/DPI/IME/ActionPreview) + PreviewSource lease prototype | 2 | Pass = enter Phase 1; fail = stop, redesign rendering |
| **1** | Controller state machine + `PreviewInput(InlineSuggestion, text)` integration + suppression rules | 5 | |
| **2** | Keyboard wiring (Tab + RightArrow + Esc + Ctrl+Esc + char dismiss with prefix-eating) | 4 | |
| **3** | StubProvider + `InjectTypedText` API + PSReadLine compat tests | 5 | Pass = enter Phase 4; fail = redesign accept path |
| **4** | Append-only enforcement + suppression rule pass + diagnostics + manual UX test sheet | 4 | |
| **5** | Settings UI + first-run hint + telemetry + AC tests | 3 | |

Total remaining: ~23 days.

### Where to find more detail
- Phase descriptions, success criteria, kill criteria: `01-plan.md`
- API contracts, threading, IDL: `02-architecture.md`
- Interaction model, defaults, settings UI mock: `03-ux.md`
- Risk register with mitigations: `04-risks.md`

---

## Top architecture decisions (reference card)

These were settled through 17+ specialist sub-agent reviews. Don't relitigate without strong reason.

1. **Rendering primitive: reuse `Terminal::PreviewText`** (renderer-native), NOT a new XAML overlay. This eliminates z-order / font-mismatch / retro-shader / DPI risks.
2. **`EditLineState.CursorPrefix` uses cursor-clipping**, not full-line + attribute filtering. PSReadLine prediction text inherits `MarkKind::Command` and cannot be filtered by attribute.
3. **Accept path**: synthesized per-character `SendCharEvent` (typed-input semantics, preserves PSReadLine undo/history/syntax-highlight/prediction-cache). NOT raw `SendInput` (paste-like). NOT bracketed paste (changes shell behavior).
4. **Tab hierarchy**: user-defined keybindings > overlay accept (only when state == `Showing` AND no shell-completion menu visible) > default Tab.
5. **PreviewSource priority**: TSF/IME composition > ActionPreview > InlineSuggestion. When higher-priority source activates, lower clears. No stacking.
6. **PSReadLine policy**: "AI wins visually while showing" — `PreviewText` pads to end-of-line, covering PSReadLine prediction cells. NOT side-by-side coexistence.
7. **Append-only contract**: provider's suggestion must extend `CursorPrefix` exactly. Controller validates and rejects mismatch.
8. **Prefix-eating persistence**: typing matching ghost first char shrinks suggestion by 1 char; only typing a divergent char dismisses.
9. **V1 = StubProvider only**, in-process, no remote LLM, no `wta` dependency. Real provider is V2 (gates privacy review).
10. **Default OFF** with kill criteria: feature-off keystroke→PTY latency must stay within `baseline+50µs` p99.

---

## Memory store (separate from branch)

The cross-session memory store is at `C:\Users\yeelam\OneDrive - Microsoft\Documents\.copilot\Memory` on the originating machine. **It auto-syncs via OneDrive** if you sign into the same Microsoft account on the new machine. No manual transfer needed.

If you sign in with a different account on the new machine, the new CLI session will start with empty memory and re-learn over time. This is fine — the branch + this `HANDOFF.md` are the canonical handoff; memory is best-effort context.

---

## Prompt to bootstrap the next agent session

Copy-paste this verbatim into the new machine's Copilot CLI as your first message:

```
I'm resuming work on the inline AI ghost-text suggestions feature on
branch `dev/vanzue/inline-agent` of the agentic-terminal repo.

Read these documents in order before doing anything:
1. doc/specs/inline-suggest/HANDOFF.md (current status + how to continue)
2. doc/specs/inline-suggest/01-plan.md (master plan, phases, kill criteria)
3. doc/specs/inline-suggest/02-architecture.md (API, threading, IDL)
4. doc/specs/inline-suggest/03-ux.md (interaction model)
5. doc/specs/inline-suggest/04-risks.md (risks + mitigations)

Phase 0 is complete and committed (4 features, 4 commits ending at 63bb69cb2).
The local build is unverified because the originating machine was missing
the Windows 11 SDK 10.0.22621.0 + .NET SDK. First task on this new machine:

  1. Run the build (cmd /c "tools\razzle.cmd && bz no_clean")
  2. If it fails with environment errors, identify what's still missing.
  3. If it builds, run `runut Control.UnitTests.dll /name:*EditLineStateTests*`
     and report which of the 10 tests pass/fail.
  4. Once the test suite is green, ask me whether to proceed to Phase 0.5
     (PreviewText suitability spike).

Process discipline:
- Always fan out parallel sub-agents for design / review / investigation.
  I dislike serial work — keep the queue full.
- Every code change goes through coder → reviewer → fix → re-review until
  the reviewer approves at the 99% senior-developer-no-concern bar.
- Commit each feature separately with the same trailer: 
  Co-authored-by: Copilot <223556219+Copilot@users.noreply.github.com>
- Update SQL todos as you progress.
- Don't relitigate the 10 architecture decisions in HANDOFF.md without
  strong new evidence — they were settled through 17 reviewers.

Phase 0 follow-up items captured (handle during Phase 0.5):
- PowerShell shell-integration script does not emit OSC 133;C; commandRunning
  derivation needs either a script update or a refined derivation.
- Latency baseline must be measured before Phase 1 entry.
- Real-pwsh PSReadLine integration test deferred from Phase 0.

Don't call task_complete on intermediate steps. Only on full task completion.
```

---

## Quick file-location cheat sheet

| Asset | Location |
|---|---|
| Code branch | `dev/vanzue/inline-agent` (pushed to origin) |
| Plan documents | `doc/specs/inline-suggest/*.md` (committed in this branch — travels with `git pull`) |
| Test code | `src/cascadia/UnitTests_Control/EditLineStateTests.cpp` |
| Touched runtime files | `src/buffer/out/textBuffer.{hpp,cpp}`, `src/cascadia/TerminalCore/Terminal.{hpp,cpp}`, `src/cascadia/TerminalControl/ControlCore.{idl,h,cpp}` |
| Memory store | `C:\Users\yeelam\OneDrive - Microsoft\Documents\.copilot\Memory` (OneDrive auto-syncs) |
| Original session state | `C:\Users\yeelam\.copilot\session-state\be9f7fce-2ff9-4609-912d-9fd862ab9a20\` (local — superseded by `doc/specs/inline-suggest/`, no need to copy) |

---

*Generated by Copilot CLI on the originating machine after Phase 0 completion. Update this file as the work progresses.*
