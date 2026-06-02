# Known Issues

This page lists known issues in the current release of Intelligent Terminal along with their workarounds. If you hit something that isn't covered here, please [file an issue](https://github.com/microsoft/intelligent-terminal/issues).

## 1. Session management may not work on the very first tab after install

**Symptom:** Immediately after installing Intelligent Terminal for the first time and selecting GitHub Copilot CLI as your agent, the **Agent Management** panel (<kbd>Ctrl+Shift+/</kbd>) may not show your active session for the very first tab you open.

**Workaround:** Either open a **second tab**, or run `/restart` inside the agent pane of the first tab. The session will then show up in Agent Management as expected.

This only affects the first tab of the first launch — subsequent tabs and subsequent app launches are unaffected.

## 2. First Run Experience (FRE) install can be slow or fail on poor networks

**Symptom:** Depending on which agent you pick, the first-run setup may need to download dependencies — [`winget`](https://learn.microsoft.com/windows/package-manager/winget/) is used to install GitHub Copilot CLI and (when needed) Node.js LTS, and `npm`/`npx` is used to fetch ACP wrappers for the bring-your-own agents (Claude, Codex, Gemini). On slow, throttled, or unreliable networks any of these downloads can take **more than 10 minutes**, and on intermittent connections they can fail outright.

**Workaround:**

- Make sure you're on a stable, unrestricted internet connection before running the FRE.
- If the FRE fails or times out, you can install the missing dependencies manually by following [`installing-dependencies.md`](./installing-dependencies.md), then re-open Intelligent Terminal — the FRE will detect what's already installed and skip those steps.

## 3. Windows 11 only in the current release

**Symptom:** Intelligent Terminal will not install on Windows 10.

**Why:** The package manifest sets `MinVersion="10.0.22621.6060"` (Windows 11 22H2), so the MSIX install is blocked on earlier OS builds.

**Workaround:** Use Windows 11 (22H2 or later). Windows 10 support is planned for a later release.

## 4. Installing a new agent CLI after the FRE doesn't auto-set-up session-tracking hooks

**Symptom:** You completed the FRE with one agent (say, Copilot), then later installed Claude or Codex (or another bring-your-own ACP-compatible CLI) and switched the **agent pane** to it in Settings. The agent pane may not work, or **Agent Management** doesn't track its sessions.

**Why:** The FRE only sets up the session-tracking hooks for the agents you went through it with. Agents installed *after* the FRE need a one-time manual setup. (The ACP wrapper itself is auto-fetched on demand via `npx`, so there is no wrapper "install" to run — see [Step 3.2.3](./installing-dependencies.md#step-323--acp-wrapper-no-install-action-required) / [Step 3.3.3](./installing-dependencies.md#step-333--acp-wrapper-no-install-action-required) — but you do need to make sure the prerequisites the wrapper depends on are in place.)

**Workaround:**

1. **Make sure the prerequisites are in place.** Follow the steps in [`installing-dependencies.md`](./installing-dependencies.md) that match your agent — install Node.js LTS and the agent's own CLI (via `npm install -g …`). The ACP wrapper itself requires no install action; it will be downloaded automatically the first time the agent is launched:
   - Claude: [Steps 3.2.1 – 3.2.3](./installing-dependencies.md#32-claude-code-bring-your-own)
   - Codex: [Steps 3.3.1 – 3.3.3](./installing-dependencies.md#33-openai-codex-bring-your-own)
   - Gemini: [Section 3.4](./installing-dependencies.md#34-gemini-cli-bring-your-own)

2. **Re-install the session-tracking hooks.** Open Intelligent Terminal **Settings → Agent**, scroll to the **Agent session tracking (hooks)** row ("Track sessions across agents. Required for agent session management."), expand it, and click the **Install hooks** button next to *Install agent hook script*. This wires the newly installed CLI into Agent Management so its sessions show up in the panel.

---

*Last updated: 2026-06-02. See the [release notes](https://github.com/microsoft/intelligent-terminal/releases) for issues fixed in newer versions.*
