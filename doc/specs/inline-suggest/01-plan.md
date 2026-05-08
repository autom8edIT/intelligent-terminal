# Inline Agent — Master Plan

**Branch:** `dev/vanzue/inline-agent`

**Goal:** Opt-in inline ghost-text command continuation in the terminal grid. As the user types at a PowerShell prompt, dim renderer-native preview text proposes a suffix that can be reviewed before anything reaches the shell. **Tab** or **RightArrow at end-of-line** accepts; **Esc** dismisses; **Enter** sends only the user's typed text.

**Scope (V1):** PowerShell on Windows Terminal only. Command-completion only. No natural-language-to-command mode, no agent pane involvement, no `wta` background process, no real provider. V1 ships only the in-proc `StubProvider` with fake delay and canned suffixes. Real AI providers and natural-language command generation are V2 gates.

---

## Success Criteria

1. User can type at a PowerShell prompt and see dim ghost text appear inline within the V1 latency budget using the existing renderer-native preview path.
2. Suggestions render through `Terminal::PreviewText` / `ControlCore::PreviewInput` with `PreviewSource::InlineSuggestion`; no new visual overlay control is introduced.
3. **Tab** accepts only under the shell-completion-first hierarchy: user keybindings win, visible or recently closed shell completion menus keep Tab, otherwise visible AI suggestions accept.
4. **RightArrow** at end-of-line accepts when a suggestion is showing, giving users a non-Tab accept path.
5. **Esc** dismisses and suppresses suggestions for the current line until Enter or Ctrl+C; **Ctrl+Esc** or `togglePauseInlineSuggestions` pauses for the current TermControl session.
6. Typing the next visible suggestion character keeps the suggestion showing and shrinks it by one character; divergent typing dismisses and restarts debounce.
7. **Enter** sends what the user actually typed, never unaccepted ghost text.
8. Feature is gated by `experimental.inlineSuggestions.enabled` (default **off**) and feature-off keystroke latency stays within the budget below.
9. `ControlCore::GetEditLineState()` provides correct `CursorPrefix` and prompt/command state, verified by 10 invariant tests before Phase 1.
10. Suggestions are append-only: provider results that do not share `EditLineState.CursorPrefix` exactly are rejected as `kind="none"`; `result.suggestion` is the suffix to append.
11. PSReadLine prediction is covered visually while AI is showing and reappears naturally after AI dismisses; the setting is `psReadLinePolicy`.
12. V1 suppresses suggestions for unsafe or visually fragile cases: mid-line cursor, quotes/message flags, fresh pipeline/control operators, paste, alt buffer, missing prompt mark, running command, newline, right-edge clipping, and append-only mismatch. Unicode code points are supported by the architecture; V1 suggestions are ASCII-only because StubProvider canned data is ASCII.
13. Accessibility: AI preview is not announced while showing; on accept, a live region announces `Accepted suggestion: <text>`.

## Kill Criteria — When We Abandon V1

These are hard gates. If any criterion is hit, stop implementation and redesign rather than continuing to patch. Document the failure cause, add it to `plan-risks.md` as a **Confirmed Blocker**, reset plans, and re-evaluate whether V1 is feasible at all.

### Phase 0 kill criteria (must pass to enter Phase 0.5)

- `EditLineState` invariant test #6 with REAL `pwsh` + PSReadLine + `HistoryAndPlugin`: if `CursorPrefix` cannot be reliably extracted (off by even 1 char in PSReadLine prediction scenarios), STOP. The append-only contract depends on this.
- `CommandRunning` derivation from mark state: if reliability is <90% across `clear`, `Ctrl+L`, multi-line continuation, and PSReadLine prediction repaint scenarios, STOP.
- Feature-OFF latency: if measurement shows >100µs p50 added to keystroke→PTY path, STOP and redesign feature gating.

### Phase 0.5 kill criteria (must pass to enter Phase 1)

- IME / TSF composition: if AI preview cannot be cleanly suppressed during active IME composition (fixing the `GetActiveComposition` order doesn't suffice), STOP.
- Action Preview coexistence: if the `PreviewSource` lease pattern doesn't cleanly arbitrate (action preview occasionally lost or stuck), STOP.
- Acrylic / retro shader / software renderer: if AI preview visually breaks in any of these modes (>1 frame visual artifact), STOP.
- Screen-reader (NVDA + Narrator) baseline: if AI preview generates announcement spam, STOP and redesign announcement policy.

### Phase 3 kill criteria (must pass to enter Phase 4)

- PSReadLine state damage: if accepting a 50-char suggestion via per-char `SendCharEvent` corrupts PSReadLine undo, history, prediction cache, or syntax-highlight state in any reproducible scenario, STOP and redesign accept path (likely require V2-quality batched paste with user warning).

## Latency Budget

| Phase | p50 | p99 | Hard cap |
|-------|-----|-----|----------|
| Keystroke→PTY (feature OFF) | baseline | baseline | baseline+50µs |
| Keystroke→PTY (ON, no suggestion) | baseline+10µs | baseline+50µs | baseline+500µs |
| Trigger→suggestion visible (stub, default 300ms debounce) | 550ms | 800ms | 1200ms |
| Tab→shell receives accepted text (50-char) | 30ms | 60ms | 100ms |

Phase 1 entry is gated on measuring the feature-OFF baseline so later work can prove no misplaced early-out regression.

## Non-Goals (V1)

- Cross-shell support (bash/zsh/cmd come later through the same edit-line and preview primitives).
- Multi-line ghost text proposals.
- New rendering surface, XAML visuals, or cursor-to-DIPs anchoring for suggestions.
- Natural-language answers or answer-as-command behavior. V2 may add an explicit `?` or mode-key path with destructive-command risk classification.
- Real AI provider, remote provider, local model provider, streaming, or multiple candidates.
- Removing the existing `?<prompt>` Command Palette flow.

## High-Level Architecture (one paragraph)

AI inline suggestions reuse `Terminal::PreviewText` / `ControlCore::PreviewInput`, the renderer-native preview path already used by Action Preview and `experimental.rainbowSuggestions`, so ghost text is painted in the same render pass as buffer text. `PreviewInput` is extended with `PreviewSource` and a priority model: `ActionPreview` wins over `InlineSuggestion`, while TSF/IME composition wins over both by inverting `IRenderData::GetActiveComposition()` preference so composition is never hidden by preview text. `TermControl::_KeyHandler` handles Tab, RightArrow, Esc, Ctrl+Esc, and keybinding precedence; `_CharacterHandler` implements prefix-eating persistence and divergent-character dismissal. Buffer state comes from `ControlCore::GetEditLineState()` including `CursorPrefix`, prompt marks, command-running state, alt-buffer state, and a paste-in-progress signal. Suggestions are fetched asynchronously from `IInlineSuggestionProvider`; V1 uses only `StubProvider`. Settings live under `experimental.inlineSuggestions.*`, including `acceptKeys` and `psReadLinePolicy`.

## Phases

| Phase | Output | Days |
|-------|--------|------|
| 0 | `EditLineState` API + 10 invariant tests + paste-in-progress flag | 4 |
| 0.5 | `PreviewText` suitability spike: dev-only command calls `TermControl::PreviewInput(L"status --porcelain", PreviewSource::InlineSuggestion)` on the active terminal; verify acrylic, retro shader, software renderer, fullscreen, resize, split panes, DPI transition, scrollback; prototype `PreviewSource` priority and the InlineSuggestion display-attributes override for dim/secondary color | 2 |
| 1 | `InlineSuggestionController` state machine + `PreviewInput` integration with `PreviewSource::InlineSuggestion` + suppression rules | 5 |
| 2 | Keyboard wiring: Tab + RightArrow + Esc + Ctrl+Esc + character dismiss with prefix-eating | 4 |
| 3 | `StubProvider` + typed-character-compatible acceptance + PSReadLine compat tests + `InjectTypedText` API | 5 |
| 4 | Append-only enforcement + suppression rule pass + diagnostics + manual UX test sheet | 4 |
| 5 | Settings UI + first-run hint + telemetry + accessibility tests + diagnostics view; gated behind Phase 0.5 + Phase 1 success, and deferred if Phase 0 / 0.5 kill criteria fail | 3 |

**Total: 27 days (~5.5 weeks).** The estimate increases slightly because the plan trades overlay work for `PreviewSource`, IME precedence, diagnostics, typed-character-compatible acceptance validation, and a stricter suppression/latency gate.

## Documents in this plan

- **`plan.md`** (this file) — master plan, phases, success criteria
- **`plan-architecture.md`** — components, data flow, interfaces, threading, extension points
- **`plan-ux.md`** — interaction model, defaults, edge cases, settings shape, telemetry events
- **`plan-risks.md`** — risk register with severity, likelihood, mitigation

## Decisions Locked Before Phase 1

1. Rendering primitive: reuse `PreviewText`; no separate visual layer.
2. Preview ownership: `PreviewSource` with `ActionPreview > InlineSuggestion` and IME composition above preview.
3. PSReadLine policy: V1 default `psReadLinePolicy: "aiWinsVisually"`.
4. Accept policy: user keybindings, then Tab with shell-completion-first checks, plus RightArrow at end-of-line.
5. Provider contract: suggestions are suffixes; prefix mismatch rejects.
6. V1 provider: `StubProvider` only.
7. V1 debounce: fixed 300ms with a config knob; adaptive debounce is V2 only.

## Ownership

Owners must be assigned before Phase 0 exits.

| Component | Owner | Notes |
|-----------|-------|-------|
| StubProvider lifecycle | `<TBD>` | Own canned suffixes, fake-delay behavior, and V1-only lifecycle. |
| Shell-integration script evolution | `<TBD>` | Currently embedded in `AppActionHandlers.cpp:1819-1887`; own prompt-mark script changes and prerequisite notice accuracy. |
| Settings UX (`AIAgents.xaml` + ViewModel) | `<TBD>` | Own Settings page, diagnostics view, and first-run notice text. |
| Telemetry schema / privacy review | `<TBD>` | Own event schema, build gating, sampling, and privacy approval. |
| Dashboard / monitoring | `<TBD>` | V2 only. |
| Post-V1 StubProvider removal | `<TBD>` | Own removal plan when the V2 real provider ships. |

## Out-of-Repo Dependencies

- None. V1 uses only the local in-proc `StubProvider`; real provider integration is V2/post-V1 and requires privacy review.

## Test Strategy

- **Unit tests**: deterministic `EditLineState` invariants, `PreviewSource` priority, controller state transitions, append-only enforcement, suppression rules, prefix-eating, accept-key hierarchy.
- **Manual / integration**: UX test sheet covering commands, shell-completion conflict, real-pwsh PSReadLine prediction, paste, quotes/message flags, pipelines, history navigation, split panes, resize, DPI, acrylic, retro shader, software renderer, accessibility announcement.
- **Perf**: Measure feature-off baseline before Phase 1; enforce the latency budget above.
- **PSReadLine compatibility**: Verify visual AI-wins behavior, Tab completion menu detection, accepted-text injection, and documented best-effort undo behavior.

## What "Done for V1" Means

A reviewer can:
1. Pull `dev/vanzue/inline-agent`, build, enable Autofix in Settings → AI Agents if shell integration is not already installed, and set `experimental.inlineSuggestions.enabled: true`.
2. Open a PowerShell tab and see the one-time first-run notice: `AI inline suggestions enabled. Press Tab or RightArrow to accept. Esc to dismiss.` If OSC 133;B is not detected within 30s, instead see the one-time prerequisite notice: `Inline Suggestions requires shell integration. Enable Autofix in Settings → AI Agents to install it (one-time setup).`
3. With shell integration detected, type `git st` and see a renderer-native `StubProvider` suffix after the configured fake delay.
4. Accept with Tab when no shell completion menu is visible or recently closed, or with RightArrow at end-of-line; accepted text reaches PSReadLine through `InjectTypedText`.
5. Press Esc and observe dismissal plus line-local suppression until Enter or Ctrl+C.
6. Type the next visible suggestion character and see the remaining suggestion shrink instead of flicker away.
7. Toggle setting off and verify the keystroke pipeline returns to the measured feature-off baseline.