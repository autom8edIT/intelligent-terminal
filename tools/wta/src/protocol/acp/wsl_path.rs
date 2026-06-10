//! Pick a usable working directory for an ACP `session/new` request.
//!
//! ## Why this exists
//!
//! WT reports a pane's cwd by inspecting the Windows-side foreground
//! process. For a WSL pane that process is `wsl.exe`, whose Windows cwd is
//! its launch directory — typically `C:\Windows\System32` when the profile
//! has no `startingDirectory`. The Linux `copilot` (or any POSIX agent)
//! launched inside the distro then rejects that path: the ACP `cwd` field
//! must be a valid absolute path *in the agent's own namespace*, and
//! `C:\WINDOWS\system32` is neither absolute-POSIX nor reachable.
//!
//! The real Linux `$PWD` of a non-shell-integrated bash is not observable
//! from Windows, so we can't recover the user's actual `cd` location. What
//! we *can* do is hand every new session a **usable** starting directory:
//!
//!   * a real Windows project path → translated into the distro via
//!     `wslpath -a` (`C:\repo\foo` → `/mnt/c/repo/foo`);
//!   * a "junk" launcher path (System32 / the WT install dir) or nothing →
//!     the distro user's `$HOME`, with `/root` as a last resort.
//!
//! For Windows-native agents the logic degrades to: pass a real path
//! through, otherwise fall back to `%USERPROFILE%`.
//!
//! ## Where it runs
//!
//! `wta-master` is the single chokepoint that both knows the configured
//! `agent_cmd` (so it can detect a WSL launcher) *and* forwards every
//! `session/new` / `session/load` to the agent CLI. It calls
//! [`resolve_session_cwd`] from a blocking task before forwarding.

use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Hard cap on any single `wsl.exe` invocation. A cold WSL VM wake adds up
/// to ~2 s; 4 s leaves headroom without letting a wedged distro hang
/// session startup. On timeout we kill the child and fall through.
const WSL_TIMEOUT: Duration = Duration::from_secs(4);

/// The two WSL queries the resolver needs. Abstracted into a trait so the
/// pure cwd-selection logic in [`resolve_session_cwd_with`] is unit-testable
/// without spawning a real `wsl.exe`.
pub(crate) trait WslResolver {
    /// Translate a Windows absolute path into the distro's POSIX namespace
    /// via `wslpath -a`. `None` on any failure (bad path, distro down, …).
    fn translate(&self, distro: Option<&str>, win_path: &Path) -> Option<PathBuf>;

    /// The distro user's `$HOME`. `None` on any failure.
    fn home_dir(&self, distro: Option<&str>) -> Option<PathBuf>;
}

/// Decide the cwd to pass to the agent process *and* to the ACP
/// `session/new` JSON body.
///
/// `raw` is whatever WT reported for the active pane (or `None`/empty when
/// WT isn't connected or the active pane is the agent pane itself).
/// `agent_cmd` is the user-configured launcher string.
pub fn resolve_session_cwd(raw: Option<&Path>, agent_cmd: &str) -> PathBuf {
    resolve_session_cwd_with(raw, agent_cmd, &RealWsl)
}

/// Pure core of [`resolve_session_cwd`], parameterized over the WSL queries.
pub(crate) fn resolve_session_cwd_with(
    raw: Option<&Path>,
    agent_cmd: &str,
    resolver: &impl WslResolver,
) -> PathBuf {
    // Drop empty / "junk launcher" paths — treat them as if WT had said None.
    let usable = raw.filter(|p| !p.as_os_str().is_empty() && !is_junk_cwd(p));

    match parse_wsl_agent(agent_cmd) {
        // WSL agent: the cwd must live in the distro's POSIX namespace.
        Some(distro) => {
            let distro = distro.as_deref();
            if let Some(path) = usable {
                // A path the shell already emitted in POSIX form (e.g. via
                // shell integration) is good as-is.
                if is_posix_absolute(path) {
                    return path.to_path_buf();
                }
                if let Some(translated) = resolver.translate(distro, path) {
                    return translated;
                }
            }
            resolver
                .home_dir(distro)
                .unwrap_or_else(|| PathBuf::from("/root"))
        }
        // Windows-native agent: real path passes through; junk → %USERPROFILE%.
        None => usable
            .map(Path::to_path_buf)
            .unwrap_or_else(user_profile_dir),
    }
}

/// Parse `agent_cmd` and, if its launcher is `wsl.exe`, return the `-d
/// <distro>` argument (or `None` for the default distro). Returns `None`
/// (the outer Option) when the agent is not a WSL launcher.
///
/// Uses the same `split_whitespace` tokenization as `spawn.rs`; quoting is
/// out of scope (documented limitation — wrap quoted agents in a script).
fn parse_wsl_agent(agent_cmd: &str) -> Option<Option<String>> {
    let mut tokens = agent_cmd.split_whitespace();
    let first = tokens.next()?;
    let leaf = first.rsplit(['\\', '/']).next().unwrap_or(first);
    if !(leaf.eq_ignore_ascii_case("wsl") || leaf.eq_ignore_ascii_case("wsl.exe")) {
        return None;
    }
    // Find `-d <distro>` (or `--distribution <distro>`). Stop at `--`, which
    // separates wsl's own flags from the command to run inside the distro.
    let mut distro = None;
    let mut prev: Option<&str> = None;
    for tok in tokens {
        if tok == "--" {
            break;
        }
        if let Some(flag) = prev.take() {
            if flag == "-d" || flag == "--distribution" {
                distro = Some(tok.to_string());
                break;
            }
        }
        if tok == "-d" || tok == "--distribution" {
            prev = Some(tok);
        }
    }
    Some(distro)
}

/// True for paths WT hands back when it couldn't determine a real cwd —
/// `C:\Windows\System32`, `C:\Windows`, and the WT/wta install directory.
/// Deliberately conservative: drive roots and `%USERPROFILE%` are
/// legitimate starting points and are **not** treated as junk.
fn is_junk_cwd(path: &Path) -> bool {
    let system_root = std::env::var_os("SystemRoot")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(r"C:\Windows"));
    let system32 = system_root.join("System32");

    if path_eq_ci(path, &system_root) || path_eq_ci(path, &system32) {
        return true;
    }
    if let Some(dir) = wt_install_dir() {
        if path_eq_ci(path, dir) {
            return true;
        }
    }
    false
}

/// Directory containing the running executable (the in-package `wta.exe`,
/// which sits next to `WindowsTerminal.exe`). Cached for the process.
fn wt_install_dir() -> Option<&'static Path> {
    static DIR: OnceLock<Option<PathBuf>> = OnceLock::new();
    DIR.get_or_init(|| {
        std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf))
    })
    .as_deref()
}

/// `%USERPROFILE%` (the sane Windows-native fallback), or the current dir if
/// the env var is somehow unset.
fn user_profile_dir() -> PathBuf {
    std::env::var_os("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// A POSIX absolute path (`/home/u`), as opposed to a Windows one. Used so an
/// already-POSIX cwd from shell integration skips `wslpath`.
fn is_posix_absolute(path: &Path) -> bool {
    let s = path.to_string_lossy();
    s.starts_with('/') && !s.contains('\\')
}

/// Case-insensitive path compare with trailing-separator normalization.
fn path_eq_ci(a: &Path, b: &Path) -> bool {
    fn norm(p: &Path) -> String {
        p.to_string_lossy()
            .trim_end_matches(['\\', '/'])
            .to_ascii_lowercase()
            .replace('/', "\\")
    }
    norm(a) == norm(b)
}

/// Real `wsl.exe`-backed resolver used in production.
struct RealWsl;

impl WslResolver for RealWsl {
    fn translate(&self, distro: Option<&str>, win_path: &Path) -> Option<PathBuf> {
        let path = win_path.to_str()?;
        let out = run_wsl(distro, &["wslpath", "-a", "--", path])?;
        Some(PathBuf::from(out))
    }

    fn home_dir(&self, distro: Option<&str>) -> Option<PathBuf> {
        static CACHE: OnceLock<Mutex<HashMap<Option<String>, PathBuf>>> = OnceLock::new();
        let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
        let key = distro.map(str::to_string);
        if let Ok(map) = cache.lock() {
            if let Some(hit) = map.get(&key) {
                return Some(hit.clone());
            }
        }
        // `$HOME` needs shell expansion, so route through `sh -c`.
        let out = run_wsl(distro, &["sh", "-c", "printf %s \"$HOME\""])?;
        if out.is_empty() {
            return None;
        }
        let home = PathBuf::from(out);
        if let Ok(mut map) = cache.lock() {
            map.insert(key, home.clone());
        }
        Some(home)
    }
}

/// Invoke `wsl.exe [-d <distro>] -- <args…>`, returning trimmed stdout on a
/// zero exit, or `None` on any failure / timeout. Never panics; never blocks
/// longer than [`WSL_TIMEOUT`].
fn run_wsl(distro: Option<&str>, args: &[&str]) -> Option<String> {
    let mut cmd = Command::new("wsl.exe");
    if let Some(d) = distro {
        cmd.arg("-d").arg(d);
    }
    cmd.arg("--").args(args);
    run_capture(cmd, WSL_TIMEOUT)
}

/// Spawn `cmd`, capture stdout, and enforce `timeout`. Hides the console
/// window (these run under a GUI process). Returns trimmed stdout on success.
fn run_capture(mut cmd: Command, timeout: Duration) -> Option<String> {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        // CREATE_NO_WINDOW — don't flash a console for the probe.
        cmd.creation_flags(0x0800_0000);
    }
    cmd.stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    let mut child = cmd.spawn().ok()?;
    let start = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                break;
            }
            Ok(None) => {
                if start.elapsed() > timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    return None;
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            Err(_) => return None,
        }
    }
    let mut buf = String::new();
    child.stdout.take()?.read_to_string(&mut buf).ok()?;
    let trimmed = buf.trim();
    if trimmed.is_empty() {
        None
    } else {
        Some(trimmed.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    /// Hand-coded resolver: a `wslpath` map plus a per-distro `$HOME`.
    struct MockWsl {
        translations: HashMap<String, PathBuf>,
        home: Option<PathBuf>,
    }

    impl WslResolver for MockWsl {
        fn translate(&self, _distro: Option<&str>, win_path: &Path) -> Option<PathBuf> {
            self.translations
                .get(&win_path.to_string_lossy().to_string())
                .cloned()
        }
        fn home_dir(&self, _distro: Option<&str>) -> Option<PathBuf> {
            self.home.clone()
        }
    }

    fn mock() -> MockWsl {
        let mut translations = HashMap::new();
        translations.insert(
            r"C:\repo\foo".to_string(),
            PathBuf::from("/mnt/c/repo/foo"),
        );
        MockWsl {
            translations,
            home: Some(PathBuf::from("/home/u")),
        }
    }

    fn resolve(raw: Option<&str>, agent: &str) -> PathBuf {
        let raw = raw.map(Path::new);
        resolve_session_cwd_with(raw, agent, &mock())
    }

    #[test]
    fn native_passthrough_real_path() {
        assert_eq!(
            resolve(Some(r"C:\repo\foo"), "pwsh.exe"),
            PathBuf::from(r"C:\repo\foo")
        );
    }

    #[test]
    fn native_junk_falls_back_to_userprofile() {
        // Force a deterministic USERPROFILE for the assertion.
        std::env::set_var("USERPROFILE", r"C:\Users\tester");
        assert_eq!(
            resolve(Some(r"C:\Windows\System32"), "pwsh.exe"),
            PathBuf::from(r"C:\Users\tester")
        );
    }

    #[test]
    fn native_none_falls_back_to_userprofile() {
        std::env::set_var("USERPROFILE", r"C:\Users\tester");
        assert_eq!(resolve(None, "pwsh.exe"), PathBuf::from(r"C:\Users\tester"));
    }

    #[test]
    fn wsl_real_path_is_translated() {
        assert_eq!(
            resolve(Some(r"C:\repo\foo"), "wsl.exe -d Ubuntu -- copilot"),
            PathBuf::from("/mnt/c/repo/foo")
        );
    }

    #[test]
    fn wsl_junk_falls_back_to_home() {
        assert_eq!(
            resolve(Some(r"C:\Windows\System32"), "wsl.exe -d Ubuntu -- copilot"),
            PathBuf::from("/home/u")
        );
    }

    #[test]
    fn wsl_none_falls_back_to_home() {
        assert_eq!(
            resolve(None, "wsl.exe -d Ubuntu -- copilot"),
            PathBuf::from("/home/u")
        );
    }

    #[test]
    fn wsl_without_distro_still_translates() {
        assert_eq!(
            resolve(Some(r"C:\repo\foo"), "wsl.exe -- copilot"),
            PathBuf::from("/mnt/c/repo/foo")
        );
    }

    #[test]
    fn wsl_already_posix_path_passes_through() {
        assert_eq!(
            resolve(Some("/home/u/proj"), "wsl.exe -d Ubuntu -- copilot"),
            PathBuf::from("/home/u/proj")
        );
    }

    #[test]
    fn wsl_home_failure_last_resort_root() {
        let resolver = MockWsl {
            translations: HashMap::new(),
            home: None,
        };
        let got = resolve_session_cwd_with(None, "wsl.exe -d Ubuntu -- copilot", &resolver);
        assert_eq!(got, PathBuf::from("/root"));
    }

    #[test]
    fn parse_wsl_agent_variants() {
        assert_eq!(parse_wsl_agent("pwsh.exe"), None);
        assert_eq!(parse_wsl_agent("copilot --acp"), None);
        assert_eq!(parse_wsl_agent("wsl.exe -- copilot"), Some(None));
        assert_eq!(
            parse_wsl_agent("wsl -d Ubuntu -- copilot"),
            Some(Some("Ubuntu".to_string()))
        );
        assert_eq!(
            parse_wsl_agent(r"C:\Windows\System32\wsl.exe -d Debian -- copilot"),
            Some(Some("Debian".to_string()))
        );
        // `-d` after `--` belongs to the inner command, not wsl.
        assert_eq!(parse_wsl_agent("wsl.exe -- copilot -d foo"), Some(None));
    }

    #[test]
    fn junk_detection_is_case_insensitive() {
        std::env::set_var("SystemRoot", r"C:\Windows");
        assert!(is_junk_cwd(Path::new(r"c:\windows\system32")));
        assert!(is_junk_cwd(Path::new(r"C:\WINDOWS\System32\")));
        assert!(is_junk_cwd(Path::new(r"C:\Windows")));
        assert!(!is_junk_cwd(Path::new(r"C:\repo\foo")));
        assert!(!is_junk_cwd(Path::new(r"C:\Users\me")));
    }
}
