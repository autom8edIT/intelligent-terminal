//! session_watcher — turn each agent CLI's on-disk session records into the
//! crate's existing [`crate::agent_sessions::SessionEvent`]s, hook-free.
//!
//! The per-CLI `classify_*` functions are the pure, testable core: they take
//! one parsed record (or, for Gemini, the rewritten snapshot) plus the
//! session key and return zero or more `SessionEvent`s. The watch loop
//! ([`watch`]) is the thin impure shell that tails files and feeds records
//! through them. Binding a discovered session to its pane lives in
//! [`bind`]; path → identity in [`discover`].

pub mod bind;
pub mod classify_claude;
pub mod classify_codex;
pub mod classify_copilot;
pub mod classify_gemini;
pub mod discover;

use std::collections::{HashMap, HashSet};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Read the bytes appended to `path` since byte offset `from`, returning the
/// decoded text and the new end offset. Used for the append-only CLIs.
pub fn read_appended(path: &Path, from: u64) -> std::io::Result<(String, u64)> {
    let mut file = std::fs::File::open(path)?;
    let len = file.metadata()?.len();
    if len <= from {
        return Ok((String::new(), len));
    }
    file.seek(SeekFrom::Start(from))?;
    let mut buf = Vec::with_capacity((len - from) as usize);
    file.take(len - from).read_to_end(&mut buf)?;
    Ok((String::from_utf8_lossy(&buf).into_owned(), len))
}

use crate::agent_sessions::{CliSource, SessionEvent};

/// One emitted event with its routing identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Emitted {
    pub cli: CliSource,
    pub key: String,
    /// Path-encoded session cwd when available (Claude only today; `None`
    /// for Copilot/Codex/Gemini). Consumed by master pane-binding.
    pub cwd: Option<PathBuf>,
    pub event: SessionEvent,
}

/// Per-file progress so we only classify new records.
#[derive(Default)]
pub(crate) struct Progress {
    /// Byte offset for append-only CLIs.
    offset: u64,
    /// Message count for Gemini's snapshot model.
    gemini_msgs: usize,
}

/// Process one changed file path into emitted events, advancing `progress`.
/// Pure w.r.t. everything except the on-disk file and the passed-in map.
pub fn process_change(path: &Path, progress: &mut HashMap<PathBuf, Progress>) -> Vec<Emitted> {
    let Some(disc) = discover::identify(path) else {
        return Vec::new();
    };
    let entry = progress.entry(path.to_path_buf()).or_default();
    let mut out = Vec::new();

    match disc.cli {
        CliSource::Gemini => {
            // Reparse the whole file; take the last non-empty snapshot line.
            let Ok(text) = std::fs::read_to_string(path) else {
                return out;
            };
            // Canonical key = header `sessionId` (the filename only carries the
            // first 8 hex chars). Fall back to the path-derived key if absent.
            let key = text
                .lines()
                .next()
                .and_then(|l| serde_json::from_str::<serde_json::Value>(l).ok())
                .and_then(|v| v.get("sessionId").and_then(|s| s.as_str()).map(str::to_string))
                .unwrap_or_else(|| disc.key.clone());
            let Some(last) = text.lines().rev().find(|l| !l.trim().is_empty()) else {
                return out;
            };
            let Ok(val) = serde_json::from_str::<serde_json::Value>(last) else {
                return out;
            };
            let (events, new_len) =
                classify_gemini::classify_snapshot(&val, &key, entry.gemini_msgs);
            entry.gemini_msgs = new_len;
            for event in events {
                out.push(Emitted { cli: disc.cli.clone(), key: key.clone(), cwd: disc.cwd.clone(), event });
            }
        }
        _ => {
            let from = entry.offset;
            let Ok((text, len)) = read_appended(path, from) else {
                return out;
            };
            if len < from {
                // File shrank/rotated — resync to the new end, drop nothing
                // further this tick.
                entry.offset = len;
                return out;
            }
            // Only consume through the last newline; a trailing partial line is
            // a record still being written — leave its bytes for the next tick.
            let consumed = text.rfind('\n').map(|i| i + 1).unwrap_or(0);
            entry.offset = from + consumed as u64;
            for line in text[..consumed].lines() {
                let line = line.trim();
                if line.is_empty() {
                    continue;
                }
                let Ok(val) = serde_json::from_str::<serde_json::Value>(line) else {
                    continue;
                };
                let events = match disc.cli {
                    CliSource::Copilot => classify_copilot::classify(&val, &disc.key),
                    CliSource::Claude => classify_claude::classify(&val, &disc.key),
                    CliSource::Codex => classify_codex::classify(&val, &disc.key),
                    _ => Vec::new(),
                };
                for event in events {
                    out.push(Emitted { cli: disc.cli.clone(), key: disc.key.clone(), cwd: disc.cwd.clone(), event });
                }
            }
        }
    }
    out
}

/// The four watched roots under the user profile.
pub fn watched_roots() -> Vec<PathBuf> {
    let home = std::env::var("USERPROFILE")
        .map(PathBuf::from)
        .unwrap_or_default();
    vec![
        home.join(".copilot").join("session-state"),
        home.join(".claude").join("projects"),
        home.join(".codex").join("sessions"),
        home.join(".gemini").join("tmp"),
    ]
}

use std::sync::mpsc::Sender;

/// Seed per-file progress to each existing session file's current end, so the
/// watcher only processes content appended *after* it starts. Without this, the
/// first `notify` event for a pre-existing historical file (which the OS can
/// deliver spuriously — e.g. an indexer/AV touch, or a delayed
/// ReadDirectoryChangesW batch) would make `process_change` replay that file's
/// entire record stream from offset 0. Each replayed record revives its
/// historical Class-B session and re-broadcasts `sessions/changed`, flooding
/// master with thousands of redundant notifications and stalling live updates.
///
/// Files created *after* the watcher starts are not seeded (not present here),
/// so their first sighting is still read from offset 0 — correctly catching a
/// new session's opening `session_meta` / `task_started` records.
pub(crate) fn seed_existing_progress_in(
    roots: &[PathBuf],
    progress: &mut HashMap<PathBuf, Progress>,
) {
    for root in roots {
        let mut stack = vec![root.clone()];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                match entry.file_type() {
                    Ok(ft) if ft.is_dir() => stack.push(path),
                    Ok(_) => {
                        let Some(disc) = discover::identify(&path) else {
                            continue;
                        };
                        let prog = if matches!(disc.cli, CliSource::Gemini) {
                            // Gemini's snapshot model counts messages, not bytes.
                            Progress {
                                offset: 0,
                                gemini_msgs: gemini_msg_count(&path),
                            }
                        } else {
                            Progress {
                                offset: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                                gemini_msgs: 0,
                            }
                        };
                        progress.insert(path, prog);
                    }
                    _ => {}
                }
            }
        }
    }
}

/// Current message count in a Gemini snapshot file (the last non-empty
/// `$set.messages` array). `0` on any read/parse failure.
fn gemini_msg_count(path: &Path) -> usize {
    let Ok(text) = std::fs::read_to_string(path) else {
        return 0;
    };
    let Some(last) = text.lines().rev().find(|l| !l.trim().is_empty()) else {
        return 0;
    };
    let Ok(val) = serde_json::from_str::<serde_json::Value>(last) else {
        return 0;
    };
    val.get("$set")
        .and_then(|s| s.get("messages"))
        .and_then(|m| m.as_array())
        .or_else(|| val.get("messages").and_then(|m| m.as_array()))
        .map(|a| a.len())
        .unwrap_or(0)
}

/// Mark/unmark a session file as **hot** — i.e. belonging to a session that is
/// currently mid-turn (WORKING/ATTENTION) and therefore the only kind of row a
/// dropped completion event could strand. A file goes hot when it emits a
/// start/notification event and cold when it emits a completion. The periodic
/// sweep re-reads only the hot set, so its cost scales with *active* sessions
/// rather than with the (mostly historical, long-idle) tracked-file count.
fn update_hot(hot: &mut HashSet<PathBuf>, path: &Path, event: &SessionEvent) {
    match event {
        SessionEvent::ToolStarting { .. } | SessionEvent::Notification { .. } => {
            hot.insert(path.to_path_buf());
        }
        SessionEvent::ToolCompleted { .. } | SessionEvent::SessionStopped { .. } => {
            hot.remove(path);
        }
        _ => {}
    }
}

/// Run [`process_change`] on `path`, update the hot-set from each emitted event,
/// and forward the event on `tx`. Returns `Err(())` once the receiver is gone so
/// the caller can stop the watch loop.
fn process_and_send(
    path: &Path,
    progress: &mut HashMap<PathBuf, Progress>,
    hot: &mut HashSet<PathBuf>,
    tx: &Sender<Emitted>,
) -> Result<(), ()> {
    for emitted in process_change(path, progress) {
        update_hot(hot, path, &emitted.event);
        if tx.send(emitted).is_err() {
            return Err(());
        }
    }
    Ok(())
}

/// Spawn a blocking `notify` watcher over the four roots. Each emitted event
/// is sent on `tx`. Runs until `tx` is dropped or the watcher errors.
///
/// Recursive mode is required: session files live several levels below each
/// root (e.g. `.codex/sessions/YYYY/MM/DD/...`).
pub fn watch(tx: Sender<Emitted>) -> notify::Result<()> {
    use notify::{RecursiveMode, Watcher};

    let (raw_tx, raw_rx) = std::sync::mpsc::channel::<notify::Result<notify::Event>>();
    let mut watcher = notify::recommended_watcher(move |res| {
        let _ = raw_tx.send(res);
    })?;
    for root in watched_roots() {
        // A missing root is fine (the user may not have that CLI) — log + skip.
        if root.exists() {
            if let Err(err) = watcher.watch(&root, RecursiveMode::Recursive) {
                tracing::warn!(
                    target: "session_watcher",
                    root = %root.display(),
                    error = %err,
                    "watch failed"
                );
            }
        }
    }

    let mut progress: HashMap<PathBuf, Progress> = HashMap::new();
    // Skip every record that already existed when we started watching — only
    // track genuinely new activity. See `seed_existing_progress_in`.
    seed_existing_progress_in(&watched_roots(), &mut progress);
    // Files whose session is currently mid-turn (WORKING/ATTENTION). See
    // `update_hot` — the periodic sweep below re-reads only these.
    let mut hot: HashSet<PathBuf> = HashSet::new();

    // notify (ReadDirectoryChangesW) is an *edge* signal, not a per-record queue:
    // it coalesces consecutive writes to one file and never guarantees a
    // notification arrives after the final write is durable. The last record of a
    // turn — Codex `task_complete` / Copilot `assistant.turn_end` (→ IDLE) — is
    // the most exposed: it's the file's last write before the session goes quiet,
    // so if its event is coalesced/missed there is no later write to re-trigger a
    // read and the row stays stuck on WORKING. Guard with a periodic catch-up
    // sweep that re-runs the incremental `process_change` — but ONLY over `hot`
    // files (sessions currently mid-turn). When nothing is working the sweep is a
    // no-op, so the cost scales with active sessions, not with the (mostly
    // historical, long-idle) tracked-file count.
    const SWEEP_INTERVAL: Duration = Duration::from_secs(3);
    loop {
        match raw_rx.recv_timeout(SWEEP_INTERVAL) {
            Ok(Ok(event)) => {
                for path in event.paths {
                    if process_and_send(&path, &mut progress, &mut hot, &tx).is_err() {
                        return Ok(()); // receiver gone
                    }
                }
            }
            Ok(Err(err)) => {
                tracing::warn!(target: "session_watcher", error = %err, "notify error");
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                let hot_paths: Vec<PathBuf> = hot.iter().cloned().collect();
                for path in hot_paths {
                    if process_and_send(&path, &mut progress, &mut hot, &tx).is_err() {
                        return Ok(()); // receiver gone
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn read_appended_returns_only_new_bytes() {
        let dir = std::env::temp_dir().join(format!("wta-watch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.jsonl");
        std::fs::write(&path, b"line1\n").unwrap();
        let (first, off1) = read_appended(&path, 0).unwrap();
        assert_eq!(first, "line1\n");
        assert_eq!(off1, 6);
        // Append more, read only the delta.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"line2\n")
            .unwrap();
        let (second, off2) = read_appended(&path, off1).unwrap();
        assert_eq!(second, "line2\n");
        assert_eq!(off2, 12);
    }

    #[test]
    fn process_change_emits_copilot_events_incrementally() {
        let dir = std::env::temp_dir()
            .join(format!("wta-pc-{}", std::process::id()))
            .join("session-state")
            .join("sess-9");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        std::fs::write(
            &path,
            b"{\"type\":\"tool.execution_start\",\"data\":{\"toolName\":\"bash\"}}\n",
        )
        .unwrap();
        let mut progress = HashMap::new();
        let first = process_change(&path, &mut progress);
        assert_eq!(first.len(), 1);
        assert!(matches!(first[0].event, SessionEvent::ToolStarting { .. }));
        // No new bytes -> no duplicate events.
        let second = process_change(&path, &mut progress);
        assert!(second.is_empty());
    }

    #[test]
    fn process_change_does_not_lose_a_partial_line() {
        let dir = std::env::temp_dir()
            .join(format!("wta-partial-{}", std::process::id()))
            .join("session-state")
            .join("sess-partial");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("events.jsonl");
        // One complete record + a half-written second record (no newline yet).
        std::fs::write(
            &path,
            b"{\"type\":\"tool.execution_start\",\"data\":{\"toolName\":\"bash\"}}\n{\"type\":\"assistant.turn",
        )
        .unwrap();
        let mut progress = std::collections::HashMap::new();
        let first = process_change(&path, &mut progress);
        assert_eq!(first.len(), 1, "only the complete line should classify");
        assert!(matches!(first[0].event, SessionEvent::ToolStarting { .. }));
        // Complete the partial record (turn_end → ToolCompleted under the
        // turn-based Copilot model).
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"_end\",\"data\":{\"turnId\":\"0\"}}\n")
            .unwrap();
        let second = process_change(&path, &mut progress);
        assert_eq!(second.len(), 1, "the completed record must now classify (not be lost)");
        assert!(matches!(second[0].event, SessionEvent::ToolCompleted { .. }));
    }

    #[test]
    fn seed_skips_preexisting_history_then_tracks_new_appends() {
        // A pre-existing Codex rollout (history) must be seeded to EOF so it is
        // NOT replayed from offset 0 — the bug that flooded master with revive
        // broadcasts. New content appended after seeding is still tracked.
        let root = std::env::temp_dir().join(format!("wta-seed-{}", std::process::id()));
        let day = root.join("2026").join("06").join("10");
        std::fs::create_dir_all(&day).unwrap();
        let path =
            day.join("rollout-2026-06-10T00-00-00-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.jsonl");
        std::fs::write(
            &path,
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n\
              {\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"shell\"}}\n",
        )
        .unwrap();

        let mut progress = HashMap::new();
        seed_existing_progress_in(&[root.clone()], &mut progress);

        // History is skipped — no replay on the first change.
        let replay = process_change(&path, &mut progress);
        assert!(
            replay.is_empty(),
            "seeded historical file must not replay, got {:?}",
            replay
        );

        // A genuinely new appended record IS processed.
        std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap()
            .write_all(b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_complete\"}}\n")
            .unwrap();
        let fresh = process_change(&path, &mut progress);
        assert_eq!(fresh.len(), 1, "new appended record must be classified");
        assert!(matches!(fresh[0].event, SessionEvent::ToolCompleted { .. }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn seed_does_not_skip_files_created_after_start() {
        // A file absent at seed time (new session created after the watcher
        // started) is not seeded, so it's read in full on first sight.
        let root = std::env::temp_dir().join(format!("wta-seed-new-{}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let mut progress = HashMap::new();
        seed_existing_progress_in(&[root.clone()], &mut progress); // empty root

        let path =
            root.join("rollout-2026-06-10T00-00-00-aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee.jsonl");
        std::fs::write(
            &path,
            b"{\"type\":\"event_msg\",\"payload\":{\"type\":\"task_started\"}}\n",
        )
        .unwrap();
        let out = process_change(&path, &mut progress);
        assert_eq!(out.len(), 1, "a new file must be read from offset 0");
        assert!(matches!(out[0].event, SessionEvent::ToolStarting { .. }));

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn hot_set_tracks_working_then_clears_on_completion() {
        // The sweep re-reads only "hot" files (sessions mid-turn). A turn start
        // marks the file hot; a mid-turn permission prompt keeps it hot; the
        // turn-completing event clears it so an idle session is never swept.
        let mut hot: HashSet<PathBuf> = HashSet::new();
        let p = PathBuf::from("rollout-x.jsonl");

        update_hot(
            &mut hot,
            &p,
            &SessionEvent::ToolStarting {
                key: "k".to_string(),
                tool_name: String::new(),
            },
        );
        assert!(hot.contains(&p), "task_started must mark the file hot");

        update_hot(
            &mut hot,
            &p,
            &SessionEvent::Notification {
                key: "k".to_string(),
                message: "permission".to_string(),
            },
        );
        assert!(hot.contains(&p), "an Attention prompt keeps the file hot");

        update_hot(
            &mut hot,
            &p,
            &SessionEvent::ToolCompleted {
                key: "k".to_string(),
            },
        );
        assert!(
            !hot.contains(&p),
            "task_complete must clear the file so an idle session isn't swept"
        );
    }
}
