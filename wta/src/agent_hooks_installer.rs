// wta/src/agent_hooks_installer.rs
//
// Auto-install the wt-agent-hooks bridge into Claude Code, Copilot CLI,
// and Gemini CLI on wta startup.
//
// Why this exists
// ===============
//
// The wta agent-pane registry transitions a session out of `IDLE` only when
// it receives `agent_event` broadcasts from the COM server. Those events
// originate from a small PowerShell bridge (`send-event.ps1`) that the
// CLI invokes through its hook system. If the user hasn't run a manual
// plugin-install step, the CLI never invokes the bridge, the registry
// stays empty, and the F2 list looks frozen.
//
// Each supported CLI loads hooks differently, so this module installs the
// bridge through the mechanism each CLI actually honors:
//
//   * Claude Code and Copilot CLI both expose a `plugin install` command
//     with marketplace-add support for local-path sources. We integrate
//     by **spawning the CLI itself** to register and install our plugin —
//     never by editing the CLI's settings/config files directly. Direct
//     edits would have to re-serialize JSONC files and would silently
//     strip header comments and any unknown user-managed fields.
//
//     Steps performed (per CLI):
//       1. Stage source files at
//          `%LOCALAPPDATA%\IntelligentTerminal\<cli>-plugin-src\wt-local\`
//          (a path *separate* from the CLI's install destination).
//       2. Spawn `<cli> plugin marketplace add <source-path>`.
//       3. Spawn `<cli> plugin install wt-agent-hooks@wt-local`.
//
//     All spawns are best-effort: failures (e.g. `<cli>.exe` not on
//     PATH, or "marketplace already added") are logged at warn/info and
//     never crash startup.
//
//     For Claude specifically: prior wta builds wrote a wta-tagged
//     `hooks` block directly into `~/.claude/settings.json`. We strip
//     that legacy block on every startup before invoking
//     `claude plugin install` so duplicate hook entries don't fire.
//
//   * Gemini CLI — written as a self-contained extension under
//     `~/.gemini/extensions/wt-agent-hooks/`. Gemini doesn't expose a
//     plugin-install equivalent that accepts local paths, so we keep
//     the on-disk extension layout for now.
//
// Each plugin folder bundles its own copy of the bridge script
// (`hooks/send-event.ps1`) so `${CLAUDE_PLUGIN_ROOT}` /
// `${extensionPath}` resolution stays inside the plugin layout.
//
// All writes are best-effort: failures are logged but do not block startup.

use std::fs;
use std::path::{Path, PathBuf};

use serde_json::{json, Value};

/// Identifies a file inside the `wt-agent-hooks` bundle. Used by
/// [`bundle::read`] to locate either a loose on-disk copy or fall
/// back to the content embedded into `wta.exe` at build time.
#[derive(Clone, Copy, Debug)]
enum BundleFile {
    /// `agent-hooks-plugin/hooks/send-event.ps1`
    SendEventPs1,
    /// `gemini-extension/gemini-extension.json`
    GeminiExtensionJson,
    /// `gemini-extension/hooks/hooks.json`
    GeminiHooksJson,
}

impl BundleFile {
    fn rel_path(self) -> &'static str {
        match self {
            Self::SendEventPs1 => "agent-hooks-plugin/hooks/send-event.ps1",
            Self::GeminiExtensionJson => "gemini-extension/gemini-extension.json",
            Self::GeminiHooksJson => "gemini-extension/hooks/hooks.json",
        }
    }

    fn embedded(self) -> &'static str {
        match self {
            Self::SendEventPs1 => EMBEDDED_SEND_EVENT_PS1,
            Self::GeminiExtensionJson => EMBEDDED_GEMINI_EXTENSION_JSON,
            Self::GeminiHooksJson => EMBEDDED_GEMINI_HOOKS_JSON,
        }
    }
}

/// Embedded fallbacks. These compile-time blobs guarantee the installer
/// can always produce a working plugin even when no loose copy of the
/// `wt-agent-hooks/` directory exists next to `wta.exe`. The runtime
/// resolver in [`bundle::read`] prefers loose files when available.
const EMBEDDED_SEND_EVENT_PS1: &str =
    include_str!("../wt-agent-hooks/agent-hooks-plugin/hooks/send-event.ps1");
const EMBEDDED_GEMINI_EXTENSION_JSON: &str =
    include_str!("../wt-agent-hooks/gemini-extension/gemini-extension.json");
const EMBEDDED_GEMINI_HOOKS_JSON: &str =
    include_str!("../wt-agent-hooks/gemini-extension/hooks/hooks.json");

mod bundle {
    //! Runtime resolution of bundled hook files.
    //!
    //! At build time, `wta.exe` embeds copies of every file the
    //! installer needs (see `EMBEDDED_*` constants in the parent
    //! module). At runtime, [`read`] prefers a loose copy of the bundle
    //! so distributors / testers can patch the hooks without rebuilding
    //! `wta.exe`. Lookup chain (first hit wins, embedded is the final
    //! fallback):
    //!
    //!   1. `WTA_HOOKS_BUNDLE_DIR` env var ΓÇö absolute path to a
    //!      `wt-agent-hooks/`-shaped directory (highest priority).
    //!   2. `<dir-of-current-exe>/wt-agent-hooks/` ΓÇö where the MSIX /
    //!      installer is expected to deposit the loose bundle next to
    //!      `wta.exe`.
    //!   3. Walk parents of `current_exe()` looking for
    //!      `wta/wt-agent-hooks/` ΓÇö dev-tree fallback that mirrors the
    //!      walk in `_ResolveWtaExePath` (TerminalSettingsEditor).
    //!   4. Embedded `include_str!` blob ΓÇö ships with the binary.

    use super::BundleFile;
    use std::borrow::Cow;
    use std::path::PathBuf;

    /// Read the contents of a bundle file. Returns owned text when
    /// loaded from a loose on-disk copy, or a borrow of the embedded
    /// fallback otherwise.
    pub(super) fn read(file: BundleFile) -> Cow<'static, str> {
        read_with_roots(file, &candidate_roots())
    }

    /// Test seam: separate the file lookup from candidate-root
    /// computation so unit tests can inject a deterministic chain
    /// without mutating process-wide env state.
    pub(super) fn read_with_roots(file: BundleFile, roots: &[PathBuf]) -> Cow<'static, str> {
        if let Some(text) = read_loose(file, roots) {
            return Cow::Owned(text);
        }
        Cow::Borrowed(file.embedded())
    }

    fn read_loose(file: BundleFile, roots: &[PathBuf]) -> Option<String> {
        for root in roots {
            let path = root.join(file.rel_path());
            match std::fs::read_to_string(&path) {
                Ok(text) => {
                    tracing::debug!(
                        target: "agent_hooks",
                        path = %path.display(),
                        "loaded bundle file from loose copy",
                    );
                    return Some(text);
                }
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => continue,
                Err(e) => {
                    tracing::warn!(
                        target: "agent_hooks",
                        path = %path.display(),
                        err = %e,
                        "failed to read loose bundle file; falling through",
                    );
                }
            }
        }
        None
    }

    /// Resolve candidate roots fresh on every call. The installer only
    /// reads ~5 files per run, so the cost (a few `parent()` hops + an
    /// `is_dir` stat) is negligible. Computing per-call also keeps tests
    /// honest: a `OnceLock` cache caused races where one test populated
    /// the chain before another test could set `WTA_HOOKS_BUNDLE_DIR`.
    fn candidate_roots() -> Vec<PathBuf> {
        let mut out = Vec::with_capacity(3);

        if let Some(env) = std::env::var_os("WTA_HOOKS_BUNDLE_DIR") {
            let p = PathBuf::from(env);
            if !p.as_os_str().is_empty() {
                out.push(p);
            }
        }

        let exe = std::env::current_exe().ok();
        if let Some(exe_dir) = exe.as_ref().and_then(|p| p.parent()) {
            out.push(exe_dir.join("wt-agent-hooks"));
        }

        if let Some(exe) = exe.as_ref() {
            let mut cursor = exe.parent().map(|p| p.to_path_buf());
            while let Some(dir) = cursor {
                let candidate = dir.join("wta").join("wt-agent-hooks");
                if candidate.is_dir() {
                    out.push(candidate);
                    break;
                }
                let parent = dir.parent().map(|p| p.to_path_buf());
                if parent.as_ref().map(|p| p == &dir).unwrap_or(true) {
                    break;
                }
                cursor = parent;
            }
        }

        out
    }
}

/// String used to tag every hook entry we manage so we can re-detect them
/// across runs and avoid duplicating entries on each wta launch.
const WTA_TAG: &str = "wt-agent-hooks";

/// Plugin name used in the Copilot plugin manifest and the
/// `enabledPlugins` map key. Must match `plugin.json` `name`.
const COPILOT_PLUGIN_NAME: &str = "wt-agent-hooks";

/// Marketplace identifier under which our plugin lives. Copilot CLI requires
/// marketplace names to be kebab-case (letters, numbers, hyphens — no
/// underscores). Used as:
///   * Folder name under `installed-plugins/<marketplace>/`.
///   * Key in `extraKnownMarketplaces` in settings.json.
///   * Suffix on `enabledPlugins` map keys (`<plugin>@<marketplace>`).
///
/// Older wta builds used `_direct` here, which Copilot CLI silently rejected
/// as a marketplace name (failing the kebab-case validator), causing the
/// plugin to never load even when the folder existed on disk.
const COPILOT_MARKETPLACE_NAME: &str = "wt-local";

/// Folder name under the marketplace folder that holds the plugin itself.
/// Copilot CLI's `plugin install` resolves the source path from
/// marketplace.json, then **copies** the plugin into a folder named after
/// the plugin's `name` field — so the canonical install destination is
/// `wt-local/<plugin-name>/`. We skip the source-folder copy step and
/// write the plugin directly to the canonical location, matching what
/// `copilot plugin list` validates against `installedPlugins[].cache_path`.
const COPILOT_PLUGIN_DIR_NAME: &str = COPILOT_PLUGIN_NAME;

/// Plugin version string written into `installedPlugins[].version`,
/// `plugin.json`, and `marketplace.json`. Bumped only when the wire format /
/// hook surface changes in a way users need to notice.
const COPILOT_PLUGIN_VERSION: &str = "0.1.0";

/// Embedded copy of the bridge script. **Loose copies (next to wta.exe
/// or under `WTA_HOOKS_BUNDLE_DIR`) take precedence** ΓÇö see
/// [`bundle::read`]. The `EMBEDDED_SEND_EVENT_PS1` constant declared
/// further up the file is the last-resort fallback baked into the
/// binary at build time from
/// `wta/wt-agent-hooks/agent-hooks-plugin/hooks/send-event.ps1`.

/// Folder name installed under `~/.gemini/extensions/` for Gemini CLI.
const GEMINI_EXTENSION_DIR_NAME: &str = "wt-agent-hooks";

/// Embedded copies of the Gemini extension files. Loose copies (next to
/// wta.exe or under `WTA_HOOKS_BUNDLE_DIR`) take precedence ΓÇö see
/// [`bundle::read`]. The `EMBEDDED_GEMINI_*` constants declared further
/// up the file are the last-resort fallback content sourced at build
/// time from `wta/wt-agent-hooks/gemini-extension/`.

/// Human-readable description used in both `plugin.json` and
/// `marketplace.json`. Kept short on purpose — Copilot CLI surfaces this
/// in `copilot plugin list` output.
const COPILOT_PLUGIN_DESCRIPTION: &str =
    "Forward CLI agent hook events to Windows Terminal for WTA display";

/// Hook event names → wta-side event-type identifier passed to the script.
/// Order mirrors `wta/wt-agent-hooks/agent-hooks-plugin/hooks/hooks.json` so the on-disk
/// behavior matches what a plugin install would have produced.
///
/// Only events Claude recognizes natively are listed here. Unknown event
/// names cause Claude to surface a "Quick safety check" warning at startup
/// asking the user how to handle the malformed settings.json — that's
/// hostile UX, so we keep this list strictly within Claude's documented
/// catalog (https://code.claude.com/docs/en/hooks). Copilot CLI accepts
/// the same set (a subset of the Claude format), so we reuse the table.
const HOOK_EVENTS: &[(&str, &str)] = &[
    ("SessionStart",      "agent.session.start"),
    ("SessionEnd",        "agent.session.end"),
    ("Notification",      "agent.notification"),
    ("UserPromptSubmit",  "agent.prompt.submit"),
    ("PreToolUse",        "agent.tool.starting"),
    ("PostToolUse",       "agent.tool.finished"),
    ("Stop",              "agent.stop"),
    ("SubagentStop",      "agent.subagent.stop"),
];

/// Top-level entry point. Run once at wta startup. Idempotent and silent on
/// failure: if a CLI isn't installed, we skip it; if its settings.json is
/// malformed, we leave it alone.
pub fn ensure_installed() {
    let Some(home) = home_dir() else {
        tracing::debug!(target: "agent_hooks", "no HOME/USERPROFILE; skipping");
        return;
    };
    ensure_installed_in(&home);
}

/// Run the installer against a specific home directory. Split out from
/// `ensure_installed` so tests can drive it with an isolated tempdir
/// without mutating `USERPROFILE`/`HOME` for the whole process.
fn ensure_installed_in(home: &Path) {
    install_for_claude(home);
    install_for_copilot(home);
    install_for_gemini(home);
}

/// Install the Gemini extension by writing the bundled
/// `wt-agent-hooks` extension into `~/.gemini/extensions/wt-agent-hooks/`.
///
/// Layout produced (matches `gemini extensions install <local-path>`):
///   ~/.gemini/extensions/wt-agent-hooks/
///     gemini-extension.json   # manifest (name + version + description)
///     hooks/
///       hooks.json            # event -> command mapping (uses
///                             # ${extensionPath} for the script path)
///       send-event.ps1        # embedded bridge script (same content as
///                             # the Claude/Copilot one — single source)
///
/// Idempotent: only writes when the on-disk content differs.
/// No-op when `~/.gemini/` is absent (Gemini CLI not installed).
fn install_for_gemini(home: &Path) {
    let gemini_dir = home.join(".gemini");
    if !gemini_dir.is_dir() {
        tracing::debug!(target: "gemini_hooks", "no ~/.gemini dir; Gemini CLI not present");
        return;
    }

    let ext_dir = gemini_dir
        .join("extensions")
        .join(GEMINI_EXTENSION_DIR_NAME);
    let hooks_dir = ext_dir.join("hooks");
    if let Err(e) = fs::create_dir_all(&hooks_dir) {
        tracing::warn!(target: "gemini_hooks", err = %e,
            "failed to create Gemini extension dir");
        return;
    }

    let manifest_path = ext_dir.join("gemini-extension.json");
    let hooks_path = hooks_dir.join("hooks.json");
    let script_path = hooks_dir.join("send-event.ps1");

    let manifest_text = bundle::read(BundleFile::GeminiExtensionJson);
    if let Err(e) = write_if_changed(&manifest_path, &manifest_text) {
        tracing::warn!(target: "gemini_hooks", err = %e,
            path = %manifest_path.display(),
            "failed to write Gemini extension manifest");
    }
    let hooks_text = bundle::read(BundleFile::GeminiHooksJson);
    if let Err(e) = write_if_changed(&hooks_path, &hooks_text) {
        tracing::warn!(target: "gemini_hooks", err = %e,
            path = %hooks_path.display(),
            "failed to write Gemini hooks.json");
    }
    let script_text = bundle::read(BundleFile::SendEventPs1);
    if let Err(e) = write_if_changed(&script_path, &script_text) {
        tracing::warn!(target: "gemini_hooks", err = %e,
            path = %script_path.display(),
            "failed to write Gemini bridge script");
    }
}

/// Install hooks for Claude Code by spawning `claude plugin install`.
///
/// Always uses Claude Code's own plugin manager — never edits
/// `~/.claude/settings.json` directly. Letting Claude manage its own
/// settings preserves any unknown / user-managed fields the user may
/// have added.
///
/// Steps:
///   1. Strip any wta-tagged top-level `hooks` block left behind by
///      pre-plugin-install wta builds (so duplicate entries don't fire).
///   2. Stage marketplace + plugin source files under
///      `%LOCALAPPDATA%\IntelligentTerminal\claude-plugin-src\wt-local\`.
///   3. Spawn `claude plugin marketplace add <source-path>`.
///   4. Spawn `claude plugin install wt-agent-hooks@wt-local`.
///
/// Idempotent: rewriting source files is a no-op when content matches;
/// the spawned commands are expected to be idempotent on Claude's side.
/// Failures (CLI not on PATH, "marketplace already added", etc.) are
/// logged but never fatal.
fn install_for_claude(home: &Path) {
    let claude_dir = home.join(".claude");
    if !claude_dir.is_dir() {
        tracing::debug!(target: "agent_hooks", "no ~/.claude dir; Claude not present");
        return;
    }

    // Round-8 cleanup: prior wta builds merged a tagged `hooks` block
    // directly into ~/.claude/settings.json. Now that we register the
    // plugin via `claude plugin install`, leaving that block in place
    // would fire each event twice — once from settings.json and once
    // from the plugin. Strip our entries on every startup.
    let settings_path = claude_dir.join("settings.json");
    if let Err(e) = cleanup_legacy_claude_hooks(&settings_path) {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            path = %settings_path.display(),
            "failed to strip legacy wta hooks from settings.json; non-fatal",
        );
    }

    let source_marketplace_dir = match claude_plugin_source_dir() {
        Some(p) => p,
        None => {
            tracing::warn!(
                target: "agent_hooks",
                "could not resolve LOCALAPPDATA; skipping Claude plugin install",
            );
            return;
        }
    };
    let source_plugin_dir = source_marketplace_dir.join(COPILOT_PLUGIN_DIR_NAME);

    if let Err(e) = write_marketplace_files(&source_marketplace_dir) {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            path = %source_marketplace_dir.display(),
            "failed to stage Claude marketplace source files",
        );
        return;
    }
    if let Err(e) = write_plugin_files(&source_plugin_dir, "claude") {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            path = %source_plugin_dir.display(),
            "failed to stage Claude plugin source files",
        );
        return;
    }

    // Hand off to Claude CLI for the actual registration + install.
    let source_path = source_marketplace_dir.to_string_lossy().into_owned();
    if let Err(e) = run_plugin_cli(
        "claude",
        &["plugin", "marketplace", "add", &source_path],
        "agent_hooks",
    ) {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            "claude plugin marketplace add failed; aborting plugin install",
        );
        return;
    }

    let plugin_ref = format!("{}@{}", COPILOT_PLUGIN_NAME, COPILOT_MARKETPLACE_NAME);
    if let Err(e) = run_plugin_cli(
        "claude",
        &["plugin", "install", &plugin_ref],
        "agent_hooks",
    ) {
        tracing::warn!(
            target: "agent_hooks",
            err = %e,
            plugin = %plugin_ref,
            "claude plugin install failed",
        );
    }
}

/// Install hooks for Copilot CLI by spawning `copilot plugin install`.
///
/// Always uses Copilot CLI's own plugin manager — never edits
/// `~/.copilot/settings.json` or `~/.copilot/config.json` directly.
/// Letting Copilot manage its own files preserves JSONC comments,
/// formatting, and any unknown fields the user may have added.
///
/// Steps:
///   1. Stage marketplace + plugin source files under
///      `%LOCALAPPDATA%\IntelligentTerminal\copilot-plugin-src\wt-local\`
///      (a path *separate* from the install destination).
///   2. Spawn `copilot plugin marketplace add <source-path>`.
///   3. Spawn `copilot plugin install wt-agent-hooks@wt-local`.
///
/// Idempotent: rewriting source files is a no-op when content matches;
/// the spawned commands are expected to be idempotent on Copilot CLI's
/// side. Failures (CLI not on PATH, "marketplace already added", etc.)
/// are logged but never fatal.
fn install_for_copilot(home: &Path) {
    let copilot_dir = home.join(".copilot");
    if !copilot_dir.is_dir() {
        tracing::debug!(target: "copilot_hooks", "no ~/.copilot dir; Copilot CLI not present");
        return;
    }

    // Source dir: where we *stage* the plugin layout that `copilot plugin
    // install` reads from. MUST be different from the install destination
    // (`~/.copilot/installed-plugins/wt-local/`) — Copilot copies source
    // → destination, and overlapping the two trips Copilot's loader.
    let source_marketplace_dir = match copilot_plugin_source_dir() {
        Some(p) => p,
        None => {
            tracing::warn!(
                target: "copilot_hooks",
                "could not resolve LOCALAPPDATA; skipping Copilot plugin install",
            );
            return;
        }
    };
    let source_plugin_dir = source_marketplace_dir.join(COPILOT_PLUGIN_DIR_NAME);

    if let Err(e) = write_marketplace_files(&source_marketplace_dir) {
        tracing::warn!(
            target: "copilot_hooks",
            err = %e,
            path = %source_marketplace_dir.display(),
            "failed to stage marketplace source files",
        );
        return;
    }
    if let Err(e) = write_plugin_files(&source_plugin_dir, "copilot") {
        tracing::warn!(
            target: "copilot_hooks",
            err = %e,
            path = %source_plugin_dir.display(),
            "failed to stage plugin source files",
        );
        return;
    }

    // Hand off to Copilot CLI for the actual registration + install.
    let source_path = source_marketplace_dir.to_string_lossy().into_owned();
    if let Err(e) = run_plugin_cli(
        "copilot",
        &["plugin", "marketplace", "add", &source_path],
        "copilot_hooks",
    ) {
        tracing::warn!(
            target: "copilot_hooks",
            err = %e,
            "copilot plugin marketplace add failed; aborting plugin install",
        );
        return;
    }

    let plugin_ref = format!("{}@{}", COPILOT_PLUGIN_NAME, COPILOT_MARKETPLACE_NAME);
    if let Err(e) = run_plugin_cli(
        "copilot",
        &["plugin", "install", &plugin_ref],
        "copilot_hooks",
    ) {
        tracing::warn!(
            target: "copilot_hooks",
            err = %e,
            plugin = %plugin_ref,
            "copilot plugin install failed",
        );
        return;
    }

    // Round-7 cleanup: a previous wta wrote files to `_direct/` (which
    // Copilot rejected as an invalid marketplace name). Remove the stale
    // folder so users don't see two copies of the plugin on disk.
    let stale = copilot_dir.join("installed-plugins").join("_direct");
    if stale.is_dir() {
        if let Err(e) = fs::remove_dir_all(&stale) {
            tracing::warn!(
                target: "copilot_hooks",
                err = %e,
                path = %stale.display(),
                "failed to remove stale _direct folder; non-fatal",
            );
        } else {
            tracing::info!(
                target: "copilot_hooks",
                path = %stale.display(),
                "removed stale _direct plugin folder",
            );
        }
    }
}

/// Resolve the staging directory passed as the `<source>` argument of
/// `copilot plugin marketplace add`. Persistent across runs so the
/// marketplace path Copilot stores in its settings.json doesn't churn.
///
/// Layout produced (matches what `copilot plugin marketplace add` expects):
///
///   %LOCALAPPDATA%\IntelligentTerminal\copilot-plugin-src\wt-local\
///     .claude-plugin\marketplace.json
///     wt-agent-hooks\
///       .claude-plugin\plugin.json
///       hooks\hooks.json
///       hooks\send-event.ps1
fn copilot_plugin_source_dir() -> Option<PathBuf> {
    let root = crate::runtime_paths::intelligent_terminal_root()?;
    Some(root.join("copilot-plugin-src").join(COPILOT_MARKETPLACE_NAME))
}

/// Resolve the staging directory passed as the `<source>` argument of
/// `claude plugin marketplace add`. Mirrors `copilot_plugin_source_dir`
/// but lives under `claude-plugin-src/` so the two CLIs don't collide.
fn claude_plugin_source_dir() -> Option<PathBuf> {
    let root = crate::runtime_paths::intelligent_terminal_root()?;
    Some(root.join("claude-plugin-src").join(COPILOT_MARKETPLACE_NAME))
}

/// Spawn `<exe>` with the given args, capture stdout/stderr for the
/// trace log, and return Err on spawn failure or non-zero exit.
///
/// Most-likely failure modes:
///   * `NotFound` — `<exe>.exe` isn't on PATH (user has the CLI's home
///     directory from a prior install but not the binary itself).
///     Logged as warn; caller skips remaining steps.
///   * Non-zero exit on `marketplace add` when the marketplace is
///     already registered — the CLI prints a message but the prior
///     registration is still valid. Logged as warn; caller treats it
///     as fatal for the current run, but the next wta startup retries.
///
/// On Windows the child is launched with `CREATE_NO_WINDOW` so it
/// doesn't briefly pop a console when wta is itself running headless
/// (e.g. invoked from the Settings UI's "Install hooks" button via
/// `wta install-hooks`).
fn run_plugin_cli(exe: &str, args: &[&str], _log_target: &str) -> std::io::Result<()> {
    // `_log_target` is reserved for the future when tracing supports
    // non-const targets; the `exe` field on each event already
    // disambiguates which CLI emitted the line.
    use std::process::Stdio;
    let mut cmd = std::process::Command::new(exe);
    cmd.args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let output = cmd.output()?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        tracing::warn!(
            target: "agent_hooks",
            exe = exe,
            args = ?args,
            stdout = %stdout.trim(),
            stderr = %stderr.trim(),
            status = ?output.status.code(),
            "plugin CLI returned non-zero exit",
        );
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            format!("{} {} exited {}", exe, args.join(" "), output.status),
        ));
    }
    tracing::info!(
        target: "agent_hooks",
        exe = exe,
        args = ?args,
        stdout = %stdout.trim(),
        "plugin CLI succeeded",
    );
    Ok(())
}

/// Return the discovered home directory. Mirrors `history_loader::home_dir`
/// so behavior is consistent between the two modules.
fn home_dir() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .or_else(|| std::env::var_os("HOME"))
        .map(PathBuf::from)
}

// ---------------------------------------------------------------------------
// Copilot plugin install — separate code path because Copilot CLI ignores
// the top-level `hooks` block and only loads hooks declared by registered
// plugins.
// ---------------------------------------------------------------------------

/// Write the marketplace catalog files (`marketplace.json`) into
/// `installed-plugins/wt-local/.claude-plugin/`. Copilot CLI's plugin
/// manager scans `extraKnownMarketplaces` and reads each
/// `<marketplace>/.claude-plugin/marketplace.json` to discover plugins.
fn write_marketplace_files(marketplace_dir: &Path) -> std::io::Result<()> {
    let claude_plugin_dir = marketplace_dir.join(".claude-plugin");
    fs::create_dir_all(&claude_plugin_dir)?;

    let marketplace_json = serde_json::to_string_pretty(&marketplace_json_value())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    write_if_changed(
        &claude_plugin_dir.join("marketplace.json"),
        &marketplace_json,
    )?;
    Ok(())
}

/// Build the `marketplace.json` document the plugin manager reads.
/// The `source: "./<plugin-folder>"` is resolved relative to the
/// marketplace folder when the CLI loads it. Identical content for
/// Claude and Copilot — both honor the `.claude-plugin` convention.
fn marketplace_json_value() -> Value {
    json!({
        "name":        COPILOT_MARKETPLACE_NAME,
        "description": "Local marketplace populated by wta",
        "owner":       { "name": "Agentic Terminal" },
        "plugins": [
            {
                "name":        COPILOT_PLUGIN_NAME,
                "description": COPILOT_PLUGIN_DESCRIPTION,
                "version":     COPILOT_PLUGIN_VERSION,
                "source":      format!("./{}", COPILOT_PLUGIN_DIR_NAME),
            }
        ],
    })
}

/// Write the plugin files (`.claude-plugin/plugin.json`,
/// `hooks/hooks.json`, `hooks/send-event.ps1`) into the plugin folder.
/// Idempotent: each file is only rewritten when its on-disk content
/// differs from what we'd produce.
///
/// **Manifest path** is `.claude-plugin/plugin.json`, NOT `plugin.json`
/// at the plugin root. Copilot's loader silently ignores a root-level
/// manifest (matching the `superpowers` plugin convention). Earlier wta
/// builds wrote to the root and the plugin never loaded.
fn write_plugin_files(plugin_dir: &Path, cli_source: &str) -> std::io::Result<()> {
    let claude_plugin_subdir = plugin_dir.join(".claude-plugin");
    let hooks_subdir = plugin_dir.join("hooks");
    fs::create_dir_all(&claude_plugin_subdir)?;
    fs::create_dir_all(&hooks_subdir)?;

    let plugin_json = serde_json::to_string_pretty(&plugin_json_value())
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    write_if_changed(&claude_plugin_subdir.join("plugin.json"), &plugin_json)?;
    let send_event_text = bundle::read(BundleFile::SendEventPs1);
    write_if_changed(&hooks_subdir.join("send-event.ps1"), &send_event_text)?;

    // Generate hooks.json from `HOOK_EVENTS`. Use `${CLAUDE_PLUGIN_ROOT}`
    // resolution so the plugin keeps working if the user moves their
    // CLI home dir (both Claude and Copilot substitute the plugin's own
    // folder for that variable). The `cli_source` flag is what the
    // bridge script keys off to tag emitted events with the right CLI.
    let hooks_json = serde_json::to_string_pretty(&plugin_hooks_json_value(cli_source))
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    write_if_changed(&hooks_subdir.join("hooks.json"), &hooks_json)?;

    // Pre-round-7 wta wrote a root-level `plugin.json` that Copilot
    // ignored. Remove it so users don't see two copies of the manifest.
    let stale_root_manifest = plugin_dir.join("plugin.json");
    if stale_root_manifest.is_file() {
        if let Err(e) = fs::remove_file(&stale_root_manifest) {
            tracing::warn!(
                target: "copilot_hooks",
                err = %e,
                "failed to remove stale root plugin.json; non-fatal",
            );
        }
    }

    Ok(())
}

/// Build the `plugin.json` manifest written into
/// `<plugin-root>/.claude-plugin/plugin.json`.
///
/// Deliberately omits a `hooks` field — Copilot's loader auto-discovers
/// `<plugin-root>/hooks/hooks.json` by convention (matches the
/// `superpowers` plugin), and the embedded reference manifest's `"hooks":
/// "hooks/hooks.json"` field has caused at least one reported parse warning
/// in the wild.
fn plugin_json_value() -> Value {
    json!({
        "name":        COPILOT_PLUGIN_NAME,
        "description": COPILOT_PLUGIN_DESCRIPTION,
        "version":     COPILOT_PLUGIN_VERSION,
        "author":      { "name": "Agentic Terminal" },
        "license":     "MIT",
        "keywords":    ["windows-terminal", "agent-hooks", "wta"],
    })
}

/// Build the `hooks.json` document the plugin loader will read.
/// Generated programmatically from `HOOK_EVENTS` so we don't ship stale
/// event names. `cli_source` (e.g. `"copilot"`, `"claude"`) is forwarded
/// to the bridge script via `-CliSource <name>` so emitted events are
/// tagged with the originating CLI.
fn plugin_hooks_json_value(cli_source: &str) -> Value {
    let mut hooks_map = serde_json::Map::new();
    for (event_name, event_id) in HOOK_EVENTS {
        hooks_map.insert(
            (*event_name).to_string(),
            json!([{
                "matcher": ".*",
                "hooks": [{
                    "type": "command",
                    "command": format!(
                        "powershell -ExecutionPolicy Bypass -File \"${{CLAUDE_PLUGIN_ROOT}}/hooks/send-event.ps1\" -CliSource {} {}",
                        cli_source, event_id,
                    ),
                }]
            }]),
        );
    }
    json!({ "hooks": Value::Object(hooks_map) })
}

/// Write `contents` to `path` only when the on-disk content differs. Skips
/// the write when unchanged so repeated startups don't churn mtimes.
fn write_if_changed(path: &Path, contents: &str) -> std::io::Result<()> {
    let needs_write = match fs::read_to_string(path) {
        Ok(existing) => existing != contents,
        Err(_) => true,
    };
    if needs_write {
        fs::write(path, contents)?;
        tracing::info!(
            target: "copilot_hooks",
            path = %path.display(),
            "wrote plugin file",
        );
    }
    Ok(())
}


/// Strip wta-tagged entries from the top-level `hooks` block of
/// `~/.claude/settings.json` (Round-8 cleanup). Pre-plugin-install wta
/// builds wrote our hook entries directly into settings.json; once the
/// plugin is installed via `claude plugin install`, leaving those
/// entries in place would fire each event twice. Idempotent: no-op if
/// there's nothing to clean.
fn cleanup_legacy_claude_hooks(settings_path: &Path) -> std::io::Result<()> {
    let text = match fs::read_to_string(settings_path) {
        Ok(t) if !t.trim().is_empty() => t,
        Ok(_) => return Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(e) => return Err(e),
    };

    let mut settings: Value = match serde_json::from_str(&text) {
        Ok(v) => v,
        Err(e) => {
            tracing::warn!(
                target: "agent_hooks",
                err = %e,
                path = %settings_path.display(),
                "settings.json malformed; leaving untouched",
            );
            return Ok(());
        }
    };

    let Some(root) = settings.as_object_mut() else {
        return Ok(());
    };
    let Some(hooks) = root.get_mut("hooks") else {
        return Ok(());
    };
    let Some(hooks_obj) = hooks.as_object_mut() else {
        return Ok(());
    };

    let mut changed = false;
    let event_names: Vec<String> = hooks_obj.keys().cloned().collect();
    for event_name in event_names {
        let Some(arr) = hooks_obj.get_mut(&event_name).and_then(|v| v.as_array_mut()) else {
            continue;
        };
        let before = arr.len();
        arr.retain(|entry| !entry_is_wta_tagged(entry));
        if arr.len() != before {
            changed = true;
        }
        if arr.is_empty() {
            hooks_obj.remove(&event_name);
        }
    }

    // If the hooks object is now empty, remove it entirely so we don't
    // leave behind a `"hooks": {}` artifact in the user's settings.
    if hooks_obj.is_empty() {
        root.remove("hooks");
        changed = true;
    }

    if !changed {
        return Ok(());
    }

    let serialized = serde_json::to_string_pretty(&settings)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::Other, e))?;
    fs::write(settings_path, serialized)?;
    tracing::info!(
        target: "agent_hooks",
        path = %settings_path.display(),
        "stripped legacy wta hooks block",
    );
    Ok(())
}

/// True iff the entry was inserted by us (any nested `command` string
/// references our bridge script or carries the WTA_TAG marker). Used by
/// `cleanup_legacy_claude_hooks` to identify our own entries during
/// migration off the direct-settings.json path.
fn entry_is_wta_tagged(entry: &Value) -> bool {
    let Some(hooks) = entry.get("hooks").and_then(|h| h.as_array()) else {
        return false;
    };
    for h in hooks {
        let Some(cmd) = h.get("command").and_then(|c| c.as_str()) else { continue; };
        if cmd.contains(WTA_TAG) || cmd.contains("send-event.ps1") {
            return true;
        }
    }
    false
}


#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    static TMP_COUNTER: AtomicUsize = AtomicUsize::new(0);

    fn unique_dir(label: &str) -> PathBuf {
        let n = TMP_COUNTER.fetch_add(1, Ordering::Relaxed);
        let pid = std::process::id();
        let p = std::env::temp_dir().join(format!("wta-hooks-{}-{}-{}", label, pid, n));
        let _ = fs::remove_dir_all(&p);
        fs::create_dir_all(&p).unwrap();
        p
    }

    // ---- helper / generator tests ---------------------------------------

    /// `bundle::read_with_roots` resolves loose copies in priority
    /// order and falls back to the embedded blob when nothing matches.
    /// We exercise the inner helper directly (rather than via
    /// [`bundle::read`]) so we don't have to mutate process-wide env
    /// state in a parallel test runner.
    #[test]
    fn bundle_read_resolves_loose_then_falls_back_to_embedded() {
        let dir = unique_dir("bundle");
        let loose_script_dir = dir.join("agent-hooks-plugin").join("hooks");
        fs::create_dir_all(&loose_script_dir).unwrap();
        let loose_marker = "# LOOSE OVERRIDE FOR TEST\n";
        fs::write(loose_script_dir.join("send-event.ps1"), loose_marker).unwrap();

        let roots = vec![dir.clone()];

        // Loose copy wins for the file present in the override dir.
        let resolved = bundle::read_with_roots(BundleFile::SendEventPs1, &roots);
        assert_eq!(
            resolved.as_ref(),
            loose_marker,
            "expected loose copy to win over embedded fallback",
        );

        // Files NOT present in the override dir fall through to embedded.
        let manifest = bundle::read_with_roots(BundleFile::GeminiExtensionJson, &roots);
        let parsed: serde_json::Value =
            serde_json::from_str(manifest.as_ref()).expect("embedded gemini manifest parses");
        assert_eq!(
            parsed.get("name").and_then(|v| v.as_str()),
            Some(GEMINI_EXTENSION_DIR_NAME),
            "embedded gemini-extension.json missing expected name",
        );

        // With an empty root list we always get the embedded fallback.
        let embedded = bundle::read_with_roots(BundleFile::SendEventPs1, &[]);
        assert!(
            embedded.as_ref().contains("send-event.ps1"),
            "embedded send-event.ps1 should contain its own banner comment",
        );
    }

    #[test]
    fn plugin_hooks_json_uses_plugin_root_variable() {
        let v = plugin_hooks_json_value("copilot");
        let s = v.to_string();
        assert!(
            s.contains("${CLAUDE_PLUGIN_ROOT}/hooks/send-event.ps1"),
            "expected ${{CLAUDE_PLUGIN_ROOT}}-relative path: {}",
            s
        );
        for (event_name, event_id) in HOOK_EVENTS {
            assert!(s.contains(event_name), "missing event name: {}", event_name);
            assert!(s.contains(event_id), "missing event id: {}", event_id);
        }
        assert!(
            s.contains("-CliSource copilot"),
            "expected -CliSource copilot in command: {}",
            s
        );
    }

    #[test]
    fn plugin_hooks_json_threads_cli_source_through() {
        let v = plugin_hooks_json_value("claude");
        let s = v.to_string();
        assert!(
            s.contains("-CliSource claude"),
            "expected -CliSource claude in command: {}",
            s
        );
        assert!(
            !s.contains("-CliSource copilot"),
            "did not expect -CliSource copilot in claude output: {}",
            s
        );
    }

    #[test]
    fn write_plugin_files_creates_layout() {
        let dir = unique_dir("plugin-files");
        write_plugin_files(&dir, "copilot").unwrap();

        let manifest = dir.join(".claude-plugin").join("plugin.json");
        let hooks = dir.join("hooks").join("hooks.json");
        let script = dir.join("hooks").join("send-event.ps1");

        assert!(manifest.is_file(), "missing plugin.json: {}", manifest.display());
        assert!(hooks.is_file(), "missing hooks.json: {}", hooks.display());
        assert!(script.is_file(), "missing send-event.ps1: {}", script.display());

        let hooks_text = fs::read_to_string(&hooks).unwrap();
        assert!(hooks_text.contains("-CliSource copilot"));

        // Idempotent: running again is a no-op (no panic, no error).
        write_plugin_files(&dir, "copilot").unwrap();
    }

    #[test]
    fn write_plugin_files_threads_cli_source_into_hooks() {
        let dir = unique_dir("plugin-files-claude");
        write_plugin_files(&dir, "claude").unwrap();
        let hooks_text = fs::read_to_string(dir.join("hooks").join("hooks.json")).unwrap();
        assert!(hooks_text.contains("-CliSource claude"));
        assert!(!hooks_text.contains("-CliSource copilot"));
    }

    #[test]
    fn write_plugin_files_removes_legacy_root_manifest() {
        let dir = unique_dir("plugin-stale");
        fs::create_dir_all(&dir).unwrap();
        let stale = dir.join("plugin.json");
        fs::write(&stale, "{\"name\":\"old\"}").unwrap();

        write_plugin_files(&dir, "copilot").unwrap();
        assert!(
            !stale.exists(),
            "expected stale root plugin.json to be removed: {}",
            stale.display()
        );
        assert!(dir.join(".claude-plugin").join("plugin.json").is_file());
    }

    #[test]
    fn write_marketplace_files_creates_catalog() {
        let dir = unique_dir("marketplace");
        write_marketplace_files(&dir).unwrap();
        let mkt = dir.join(".claude-plugin").join("marketplace.json");
        assert!(mkt.is_file(), "missing marketplace.json: {}", mkt.display());
        let v: Value = serde_json::from_str(&fs::read_to_string(&mkt).unwrap()).unwrap();
        assert_eq!(v.get("name").and_then(|x| x.as_str()), Some(COPILOT_MARKETPLACE_NAME));
        let plugins = v.get("plugins").and_then(|x| x.as_array()).unwrap();
        assert_eq!(plugins.len(), 1);
        assert_eq!(
            plugins[0].get("name").and_then(|x| x.as_str()),
            Some(COPILOT_PLUGIN_NAME)
        );
    }

    // ---- cleanup_legacy_claude_hooks ------------------------------------

    #[test]
    fn cleanup_legacy_claude_hooks_noop_when_file_missing() {
        let dir = unique_dir("cleanup-missing");
        let path = dir.join("settings.json");
        cleanup_legacy_claude_hooks(&path).unwrap();
        assert!(!path.exists());
    }

    #[test]
    fn cleanup_legacy_claude_hooks_removes_wta_entries() {
        let dir = unique_dir("cleanup-removes");
        let path = dir.join("settings.json");
        let before = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": ".*",
                        "hooks": [{
                            "type": "command",
                            "command": "powershell -ExecutionPolicy Bypass -File \"C:\\\\foo\\\\send-event.ps1\" -CliSource claude agent.session.start"
                        }]
                    },
                    {
                        "matcher": ".*",
                        "hooks": [{
                            "type": "command",
                            "command": "echo user-defined hook"
                        }]
                    }
                ]
            },
            "model": "sonnet"
        });
        fs::write(&path, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        cleanup_legacy_claude_hooks(&path).unwrap();

        let after: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        // Unrelated key preserved.
        assert_eq!(after.get("model").and_then(|v| v.as_str()), Some("sonnet"));
        // User-defined hook preserved.
        let arr = after
            .get("hooks")
            .and_then(|h| h.get("SessionStart"))
            .and_then(|v| v.as_array())
            .unwrap();
        assert_eq!(arr.len(), 1);
        let cmd = arr[0]
            .get("hooks")
            .and_then(|h| h.as_array())
            .unwrap()[0]
            .get("command")
            .and_then(|c| c.as_str())
            .unwrap();
        assert_eq!(cmd, "echo user-defined hook");
    }

    #[test]
    fn cleanup_legacy_claude_hooks_strips_empty_hooks_object() {
        let dir = unique_dir("cleanup-empty");
        let path = dir.join("settings.json");
        let before = serde_json::json!({
            "hooks": {
                "SessionStart": [
                    {
                        "matcher": ".*",
                        "hooks": [{
                            "type": "command",
                            "command": "powershell -ExecutionPolicy Bypass -File \"C:\\\\foo\\\\send-event.ps1\" -CliSource claude agent.session.start"
                        }]
                    }
                ]
            }
        });
        fs::write(&path, serde_json::to_string_pretty(&before).unwrap()).unwrap();

        cleanup_legacy_claude_hooks(&path).unwrap();

        let after: Value = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert!(
            after.get("hooks").is_none(),
            "expected empty hooks object to be removed: {}",
            after
        );
    }

    #[test]
    fn cleanup_legacy_claude_hooks_idempotent_on_clean_file() {
        let dir = unique_dir("cleanup-clean");
        let path = dir.join("settings.json");
        let before = serde_json::json!({ "model": "sonnet" });
        let serialized = serde_json::to_string_pretty(&before).unwrap();
        fs::write(&path, &serialized).unwrap();

        cleanup_legacy_claude_hooks(&path).unwrap();

        // File should not have been rewritten (content identical).
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(after, serialized);
    }

    #[test]
    fn cleanup_legacy_claude_hooks_skips_malformed_json() {
        let dir = unique_dir("cleanup-malformed");
        let path = dir.join("settings.json");
        fs::write(&path, "{ this is not valid json").unwrap();

        // Must not panic; must not rewrite the file.
        cleanup_legacy_claude_hooks(&path).unwrap();
        let after = fs::read_to_string(&path).unwrap();
        assert_eq!(after, "{ this is not valid json");
    }

    // ---- Gemini extension layout ----------------------------------------

    #[test]
    fn install_for_gemini_writes_full_extension_layout() {
        let home = unique_dir("gemini-home");
        fs::create_dir_all(home.join(".gemini")).unwrap();

        install_for_gemini(&home);

        let ext_dir = home
            .join(".gemini")
            .join("extensions")
            .join(GEMINI_EXTENSION_DIR_NAME);
        assert!(ext_dir.is_dir(), "missing ext dir: {}", ext_dir.display());
        assert!(ext_dir.join("gemini-extension.json").is_file());
        assert!(ext_dir.join("hooks").join("hooks.json").is_file());
        assert!(ext_dir.join("hooks").join("send-event.ps1").is_file());
    }

    #[test]
    fn install_for_gemini_is_noop_when_gemini_not_installed() {
        let home = unique_dir("gemini-absent");
        // .gemini deliberately missing.
        install_for_gemini(&home);
        assert!(!home.join(".gemini").exists());
    }
}
