# Inline Agent — Architecture

## Component Diagram

Inline suggestions are not a new visual tree element. They reuse the renderer-native preview path.

```
┌─────────────────────────────────────────────────────────────────┐
│ TermControl.cpp                                                 │
│  - _KeyHandler: keybindings, Tab, RightArrow, Esc, Ctrl+Esc     │
│  - _CharacterHandler: prefix-eating and divergent dismiss       │
│  - PreviewInput(text, PreviewSource::InlineSuggestion)          │
│  - PreviewTextAnnouncement policy depends on PreviewSource      │
└─────────────────────────────────────────────────────────────────┘
        ▲ state changes / keys                 │ preview request
        │                                      ▼
┌─────────────────────────────────────────────────────────────────┐
│ InlineSuggestionController                                      │
│  - Debounce, cancellation, generation IDs                       │
│  - Suppression rules                                            │
│  - Append-only validation                                       │
│  - PreviewSource priority handoff                               │
└─────────────────────────────────────────────────────────────────┘
        ▲ edit-line state                      │ provider request
        │                                      ▼
┌─────────────────────────────────────────────────────────────────┐
│ ControlCore.cpp / Terminal.cpp                                  │
│  - GetEditLineState()                                           │
│  - Paste-in-progress + 500ms post-paste suppression             │
│  - PreviewInput(text, source) → Terminal::PreviewText           │
│  - IRenderData::GetActiveComposition() prefers IME over preview │
└─────────────────────────────────────────────────────────────────┘
        ▲                                      │ renderer-native text
        │                                      ▼
┌─────────────────────────────────────────────────────────────────┐
│ Renderer pass                                                    │
│  - Buffer text, TSF/IME composition, Action Preview, AI preview │
│  - Same font, DPI, shaders, acrylic, resize behavior as grid    │
└─────────────────────────────────────────────────────────────────┘
        ▲
        │ async V1 only
        ▼
┌─────────────────────────────────────────────────────────────────┐
│ IInlineSuggestionProvider                                       │
│  V1: StubProvider — in-proc canned suffixes + fake delay        │
│  V2+: real/local/remote providers after privacy review          │
└─────────────────────────────────────────────────────────────────┘
```

## New Files / Modifications

### New

| File | Purpose |
|------|---------|
| `src/cascadia/TerminalControl/IInlineSuggestionProvider.idl` | Provider contract returning suffix-only `SuggestionResult` |
| `src/cascadia/TerminalControl/InlineSuggestionController.{h,cpp}` | Debounce, cancellation, suppression, append-only validation, preview ownership |
| `src/cascadia/TerminalControl/InlineSuggestionState.h` | State enum + transitions |
| `src/cascadia/InlineSuggestion/StubProvider.{h,cpp}` | V1 provider: local in-proc canned suffixes with parameterized fake delay |
| `src/cascadia/UnitTests_TerminalControl/InlineSuggestionTests.cpp` | Controller, suppression, key policy, append-only tests |

### Modified

| File | Change |
|------|--------|
| `src/cascadia/TerminalControl/TermControl.{h,cpp,idl}` | Wire controller; route Tab, RightArrow, Esc, Ctrl+Esc, printable chars; call `PreviewInput(..., PreviewSource::InlineSuggestion)` |
| `src/cascadia/TerminalControl/ControlCore.{h,cpp,idl}` | Expose `GetEditLineState()`, `EditLineStateChanged`, paste-in-progress tracking, `PreviewInput(text, source)`, `InjectTypedText` |
| `src/cascadia/TerminalCore/Terminal.{h,cpp}` | Extend `PreviewText` with `PreviewSource`; enforce source priority; prefer TSF/IME composition over preview |
| `src/cascadia/TerminalCore/IRenderData.*` | `GetActiveComposition()` returns IME/TSF composition ahead of snippet preview |
| `src/cascadia/TerminalSettingsModel/MTSMSettings.h` | Add `experimental.inlineSuggestions.*` settings |
| `src/cascadia/TerminalSettingsEditor/AIAgents.xaml` | Add settings, diagnostics, first-run/test controls |
| `src/cascadia/TerminalSettingsEditor/AIAgentsViewModel.{h,cpp,idl}` | Bind settings and diagnostics |

## Preview Ownership Model

`Terminal::PreviewText` / `ControlCore::PreviewInput` currently has a single snippet-preview slot shared by Action Preview and rainbow suggestions. AI inline suggestions add another producer, so `PreviewInput` accepts a source:

```cpp
enum class PreviewSource
{
    ActionPreview,
    InlineSuggestion,
};
```

`experimental.rainbowSuggestions` remains a style attribute, not a source. Priority order (highest first), enforced in `IRenderData::GetActiveComposition()`:

1. **TSF/IME composition** (`tsfPreview` non-empty)
2. **Action Preview** (`PreviewSource::ActionPreview`)
3. **Inline Suggestion** (`PreviewSource::InlineSuggestion`)

When a higher-priority source activates, lower-priority sources are cleared, not stacked. When the higher source clears, lower-priority sources must request renewal; there is no automatic restore. `IRenderData::GetActiveComposition()` must be adjusted because the current implementation prefers `snippetPreview` over `tsfPreview`; this plan inverts that precedence for IME safety. When Action Preview clears, AI may request renewal after 300ms.

## State Machine

```
        ┌──────────┐
   ┌───►│  Idle    │◄─────────────┐
   │    └────┬─────┘              │
   │         │ fixed debounce     │ Esc / Ctrl+C / Enter /
   │         │ AND not suppressed │ focus loss / mismatch
   │         ▼                    │
   │    ┌──────────┐              │
   │    │ Fetching │──────────────┤
   │    └────┬─────┘              │
   │         │ provider returns   │
   │         │ suffix is valid    │
   │         ▼                    │
   │    ┌──────────┐              │
   │    │ Showing  ├──────────────┤
   │    └────┬─────┘              │
   │         │ Tab / RightArrow   │
   │         ▼                    │
   └────┤ Accepted │              │
        └──────────┘              │
```

Showing-state character policy:
- If the typed character equals the first visible suggestion character, pass the character to the shell and shrink the suggestion by one character.
- If it diverges, pass the character to the shell, clear preview, and restart debounce.

`Esc` while showing clears preview and suppresses the current edit line until Enter or Ctrl+C. `Ctrl+Esc` or `togglePauseInlineSuggestions` disables suggestions for this TermControl session until restart.

## Resilience & Hot-Reload Contract

All controller code follows this contract.

### No-throw boundary

Every public method on `InlineSuggestionController` is `noexcept`. Internal exceptions are caught, logged to a diagnostics ring buffer (last 16 events), and the controller continues. Three consecutive same-call exceptions auto-disable the controller for the current `TermControl` session (until restart) and surface in Diagnostics with the disable reason.

### Settings hot-reload contract

- `enabled` toggled OFF: cancel pending request (Cancel + drop late results by id); clear preview if `Showing`; release line-pause; deactivate `EditLineStateChanged` subscription; controller transitions to a `Disabled` terminal state; no further work.
- `enabled` toggled ON: re-subscribe; reset state machine to `Idle`; do NOT auto-show on the current edit (wait for next `EditLineStateChanged`).
- `debounceMs` changed while in `Debouncing`: re-arm timer with the NEW value (do not preserve old).
- `psReadLinePolicy` changed: if currently `Showing`, dismiss + restart trigger to apply the new policy.
- `acceptKeys` changed: takes effect immediately for the next keystroke.
- Provider changed (V2): cancel + drop in-flight request; new request goes to the new provider.

### Failure modes

- `GetEditLineState()` throws → log; controller treats as `HasPromptMark=false` → suppress.
- `PreviewInput()` throws → log; transition to `Idle`; do NOT retry on same input.
- Provider `SuggestAsync` throws → catch; treat as `Kind=None`; emit telemetry event `InlineSuggestion_ProviderError`.
- Provider timeout (5s) → cancel; treat as `Kind=None`.
- Late provider result after settings disabled → drop silently (id mismatch).
- Late provider result after `TermControl` destroyed → `weak_ref.get()` returns null → drop silently.

## Buffer Read API — `EditLineState`

`TextBuffer::_commandForRow` already supports cursor-clipping with `clipAtCursor=true`, which is the safe way to exclude PSReadLine ghost prediction text. Prediction cells inherit `MarkKind::Command` from `StartCommand()`, so V1 must not try to distinguish typed text from prediction by walking Command-marked runs or by attribute filtering. Phase 0 adds a public snapshot method that returns only the command text up to the cursor and projects it through `ControlCore::GetEditLineState()`.

```cpp
struct EditLineSnapshot
{
    std::wstring cursorPrefix; // Prompt mark through cursor, equivalent to _commandForRow(..., clipAtCursor=true).
    bool cursorAtEnd;
    bool hasPromptMark;
    bool commandRunning;
    bool inAltBuffer;
};

EditLineSnapshot CurrentEditLineSnapshot() const;
```

```idl
struct EditLineState
{
    String CursorPrefix;       // text from prompt mark up to cursor (clipAtCursor=true). Excludes ghost text by clipping.
    Boolean CursorAtEnd;       // true iff cursor is at the end of typed Command-marked text on this line
    Boolean HasPromptMark;
    Boolean CommandRunning;
    Boolean InAltBuffer;
};
EditLineState GetEditLineState();
event Windows.Foundation.TypedEventHandler<Object, Object> EditLineStateChanged;
```

Implementation details:

- `CurrentEditLineSnapshot()` uses cursor-clipping, not a post-filtered full-line extraction. `CursorPrefix` is sufficient for sending input to the provider, validating append-only suggestions, and parsing suppression heuristics.
- Wrapped rows concatenate deterministically in top-to-bottom row order. Each row contributes only the clipped Command span that is before or at the cursor; no delimiter is inserted for a soft wrap.
- `CursorAtEnd` is derived from buffer state, not from any extracted full text length: it is true iff there are no more `MarkKind::Command` cells after the cursor on the same row or in subsequent wrapped rows up to the next non-Command mark. This provides a deterministic end-of-typed-command flag without extracting prediction text.
- `CommandRunning` is derived from prompt-mark state, not from a dedicated running marker. Walk `GetMarkExtents()` backward to find the latest prompt/input marks. If the current/latest prompt has an OSC 133;A mark, has a subsequent OSC 133;B input-start mark, and no newer OSC 133;A mark has appeared, then command running is true when the cursor is on a row after the latest `;B` mark's command region. In practice, if the latest mark has `commandEnd` set or cells past `commandEnd` are non-Command, the command has started running and no new prompt has arrived, so `CommandRunning = true`; if the cursor remains inside the editable Command region, `CommandRunning = false`.
- Multi-line continuation prompts are a V1 limitation. V1 reports `hasPromptMark = false` for continuation prompt state so inline suggestions suppress; V2 may add continuation-aware prompt grouping.
- History-search mode (`Ctrl+R`) gets no special detection in V1. The API returns the visible clipped edit text, and suppression applies via the standard cursor/edit-line rules.
- `Terminal::_inAltBuffer()` is promoted to public state through `ICoreState`. `ControlCore::PasteText` sets a `_pasteInProgress` flag outside `EditLineState` and records paste completion time so suggestions suppress during paste and for 500ms afterward.

### Phase 0 Invariant Tests

| # | Scenario | Deterministic invariant |
|---|----------|-------------------------|
| 1 | Type `git status`, cursor at end | `CursorPrefix == "git status"`, `CursorAtEnd == true`, `HasPromptMark == true` |
| 2 | Type `BarBar`, cursor-left twice | `CursorPrefix == "BarB"`, `CursorAtEnd == false` |
| 3 | Empty prompt after OSC 133;B | `CursorPrefix == ""`, `CursorAtEnd == true`, `HasPromptMark == true` |
| 4 | After Enter, command running, no new prompt yet | `CommandRunning == true` (derived from "B exists, no newer A, cursor below command region"); controller suppresses suggestions with reason `commandRunning` |
| 5 | Alt buffer active | `InAltBuffer == true`; controller suppresses suggestions with reason `altBuffer` |
| 6 | Real-pwsh PSReadLine integration test: launch `pwsh`, run `Set-PSReadLineOption -PredictionSource HistoryAndPlugin`, seed with `Add-History "BarBar"`, then type `BarB` so PSReadLine renders ghost `ar` after the cursor | `CursorPrefix == "BarB"` and `CursorAtEnd == true`. This is a manual/integration test because unit tests cannot simulate real PSReadLine prediction rendering. |
| 7 | Wrapped command across two rows | `CursorPrefix` equals the top-to-bottom clipped command prefix, with no inserted newline and with trailing spaces preserved before the cursor |
| 8 | Multi-line continuation prompt | V1 returns `HasPromptMark == false`; controller suppresses suggestions with reason `missingPromptMark` |
| 9 | History search active (`Ctrl+R`) | API returns the visible clipped edit text, does not throw, and normal cursor/edit-line suppression rules decide whether suggestions show |
| 10 | Corrupted or missing prompt mark | `HasPromptMark == false`, no throw, and controller suppresses suggestions with reason `missingPromptMark` |

## Suppression Rules

Controller returns `show=false` when any rule applies:

- Cursor is not at end of edit line.
- Inside open single or double quote, using unmatched quote counts in `CursorPrefix`.
- After known message flags `-m`, `--message`, `-c`, or `--body` followed by space + opening quote.
- Immediately after `|`, `&&`, `||`, or `;` until at least 2 chars typed after the operator.
- During bracketed paste or within 500ms of paste completion.
- In alternate screen buffer.
- No `OSC 133;B` prompt mark has been seen.
- `CommandRunning == true` from the prompt-mark derivation above.
- Suggestion contains newline. Unicode code points are allowed; V1's ASCII-only behavior is a StubProvider data limitation, not a suppression rule.
- Cursor is within `suggestion.length` cells of the viewport right edge.
- Provider result fails the append-only contract.

When a suggestion is suppressed, inline AI is hidden rather than disabled: the controller remains alive, no user-facing error is broadcast, and diagnostics records `suppressed: <reason>`.

## Provider Contract

V1 has no external provider dependency. `StubProvider` is in-proc, returns canned suffixes after configurable fake delay, and never sends raw text outside the process.

```idl
struct SuggestionResult
{
    String kind;        // "suffix" or "none"
    String suggestion;  // suffix to append after EditLineState.CursorPrefix, never a full replacement line
};
```

The controller validates append-only semantics. If a provider internally produces a full-line candidate, it must start with `EditLineState.CursorPrefix`; the controller strips the prefix and displays only the suffix. If it does not start with the current cursor prefix exactly, the result is rejected as `kind="none"`.

Adaptive debounce is V2 only: 300ms after pause >1s and immediate on whitespace token boundary. V1 uses fixed 300ms with `debounceMs`.

## Key Routing and Accept Injection

### Tab / Shell Completion Ownership Boundary

Decision: AI accept handling stays at the `TermControl` level. Reason: keystroke latency. Routing every Tab through `TerminalPage` would add dispatcher hops on the hot key path.

`SuggestionsControl` is a global single XAML instance, but it is logically owned by the most recent `TermControl` that opened it. V1 uses explicit owner tracking plus a fast read-only handoff:

1. `TerminalPage` stores the control that opened `SuggestionsControl` in `_shellCompletionMenuOwner` (`winrt::weak_ref<TermControl>`). `_OpenSuggestions` sets it after computing the sender control.
2. When `SuggestionsControl` opens, `TerminalPage` calls `_shellCompletionMenuOwner.get()->SetShellCompletionMenuVisible(true)`.
3. On collapse, dismiss, empty results, or focus loss, `TerminalPage` clears the flag on the recorded owner with `SetShellCompletionMenuVisible(false)` and clears `_shellCompletionMenuOwner`.
4. On focus move to a different control, `TerminalPage` clears the flag on the previous owner before setting any new owner.
5. `TermControl` caches `_shellCompletionMenuVisible` and `_shellCompletionMenuLastVisibleAt` (`std::chrono::steady_clock::time_point`). `_KeyHandler` reads these directly: no dispatcher hop, no lock, and no XAML query in the keystroke path.

Alternative considered: have `TerminalPage` read or influence `_KeyHandler` decisions through a shared event. This is not preferred for V1 because it makes ownership less direct and risks adding synchronization work to the key path.

Data flow:

```text
TerminalPage::_OpenSuggestions
        │ records _shellCompletionMenuOwner
        ▼
TermControl::SetShellCompletionMenuVisible(true)
        │ caches bool + _shellCompletionMenuLastVisibleAt
        ▼
TermControl::_KeyHandler(Tab) reads _shellCompletionMenuVisible and timestamp
        │
        ├─ AI Showing && menu not visible/recently visible → accept AI suffix, handled
        └─ otherwise → fall through to default Tab handling / shell completion
```

Tab routing logic in `TermControl::_KeyHandler`:

```cpp
const auto menuRecentlyVisible = _shellCompletionMenuVisible ||
    (std::chrono::steady_clock::now() - _shellCompletionMenuLastVisibleAt) < 100ms;

if (key == Tab && state == Showing && !menuRecentlyVisible)
{
    acceptSuggestion();
    return handled;
}
if (key == Tab)
{
    // Fall through to default Tab handling so the shell completion menu keeps ownership.
}
```

Conservative race policy: if `_shellCompletionMenuVisible` was last set to `true` within the last 100ms, Tab does NOT accept an AI suggestion even if the flag is now `false`. This prevents the menu-just-closed race where the user pressed Tab expecting menu interaction. RightArrow remains the safe accept path whenever there is uncertainty about menu state.

Priority:
1. User-defined keybindings always win.
2. Tab while `Showing` accepts only if no immediate shell tab-completion candidate/menu is visible or recently visible.
3. Tab while a shell completion menu is visible or within the 100ms close-transition safety window goes to the shell and AI dismisses.
4. RightArrow at end-of-line while `Showing` accepts.
5. Otherwise default terminal behavior.

### Acceptance Injection and Undo

V1: ALWAYS use per-character `SendCharEvent` injection regardless of suggestion length. Drop any batched-accept threshold setting. Pre-Phase-3 gate: validate against PSReadLine for 100-char suggestions; if the 100ms hard cap is exceeded, V2 may add batching.

V1 acceptance is typed-character-compatible insertion. Because per-character `SendCharEvent` injection makes PSReadLine see N typed chars for an N-char suggestion, `Ctrl+Z` may un-type one character at a time. Fallback: V1 exposes a **Pause inline suggestions for this command** gesture: pressing Esc while a suggestion is showing dismisses it and arms suppression for the current line until Enter or Ctrl+C. If undo becomes annoying, the user can press Ctrl+C to clear the line entirely.

V2 investigates PSReadLine's `BeginUndoGroup` PowerShell binding and shell-side cooperation. If a public mechanism is found, V2 may add atomic undo; until then, V1 remains honest about per-character undo behavior.

## Rendering Scope

Because preview text is rendered by the same pass as buffer text, it inherits font, DPI, resize, acrylic, retro shader, software-renderer, split-pane, fullscreen, and scrollback behavior. The Phase 0.5 suitability spike verifies this with a dev-only command before controller work proceeds.

Phase 0.5 also extends `PreviewText` with an optional `IDisplayAttributesOverride`-style parameter so `PreviewSource::InlineSuggestion` can request a dim/secondary color (40% alpha foreground or `secondaryForeground` brush). ActionPreview keeps the existing italic styling. Estimated implementation cost: ~10-15 lines in `Terminal.cpp` plus ~5 lines in IDL.

**V1 ASCII-only is a STUB-PROVIDER limitation, not a feature limitation.** The StubProvider returns ASCII-only suggestions because its canned output is ASCII. The ARCHITECTURE supports any code points. V2's real provider can return any Unicode (CJK, emoji, RTL); the architecture passes them through PreviewText which already handles Unicode (`Terminal.cpp:1630-1633`). The "suppress non-ASCII" guard is removed from the suppression rules; instead, V1 is documented as: "Suggestions in V1 are limited to ASCII because StubProvider is the only provider. V2 with real provider unlocks full Unicode."

V1 still limits suggestion content to single-line suffixes and hides suggestions near the viewport right edge. V2 can expand multi-line content support once renderer and shell behaviors are proven.

## Settings Surface

```jsonc
{
    "experimental.inlineSuggestions.enabled": false,
    "experimental.inlineSuggestions.debounceMs": 300,
    "experimental.inlineSuggestions.maxLength": 200,
    "experimental.inlineSuggestions.acceptKeys": ["tab", "rightArrow"],
    "experimental.inlineSuggestions.psReadLinePolicy": "aiWinsVisually" // suppressIfShellPredicting | aiAlwaysSuppresses
}
```

Settings are global for V1. Per-profile behavior can be added later.

## Diagnostics Surface

Settings UI diagnostics show:
- Provider name and status, e.g. `StubProvider active` or `StubProvider unavailable`.
- Shell integration status: `detected` after OSC 133;B is observed, otherwise `not detected`.
- Last three outcomes: `triggered`, `shown`, `stale`, `suppressed: <reason>`, or `error`.
- Last suggestion latency.
- A **Test inline suggestion** button that invokes the stub path against the active terminal.

## Instrumentation Plan

Phase 0 GATE: measure the feature-OFF baseline and document it in `plan/files/baseline-perf.md` before Phase 1 work proceeds.

### Build flag

ALL inline-suggestion telemetry events compile out in shipping builds unless `INLINE_SUGGESTIONS_VERBOSE_TELEMETRY` is defined. Default for shipping = OFF.

### Production-safe events

Always compiled in, low-frequency, attributable events:

- `InlineSuggestion_FeatureEnabled` / `InlineSuggestion_Disabled` (1 per setting toggle).
- `InlineSuggestion_Triggered` (1 per provider call).
- `InlineSuggestion_Shown` (1 per render).
- `InlineSuggestion_Accepted` / `InlineSuggestion_Dismissed` (1 per terminal event).
- `InlineSuggestion_ProviderError` (rate-limited to 1/min/instance).
- `InlineSuggestion_ControllerAutoDisabled` (rare; 1 per occurrence).

### Dev-only events

Compile out in shipping; these exist ONLY behind the `INLINE_SUGGESTIONS_VERBOSE_TELEMETRY` build flag for Phase 0/1 perf measurement and are documented as removed before V1 ships:

- `_KeyHandler` entry/exit.
- `_TrySendKeyEvent` entry/exit.
- `_CharacterHandler` entry/exit.
- `SendCharEvent` entry/exit.

### Sampled counters

Lightweight counters:

- Number of triggers per minute.
- Number of suggestions shown per minute.
- Number of stale-discards per minute.
- Average debounce-to-show latency.

No raw text or suggestion text is captured. For guarded one-shot telemetry, mimic the real pattern in `src/cascadia/TerminalCore/TerminalApi.cpp:205-215` rather than non-telemetry action-handler code.

## Backward Compatibility

- Setting default **off** gives zero behavior change.
- Existing `?<prompt>` Command Palette flow remains unchanged.
- Existing wta agent pane remains unchanged and unused by V1 inline suggestions.
- New edit-line and preview-source APIs are additive except for IME precedence correction, which is an accessibility/IME correctness fix.