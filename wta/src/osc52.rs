//! OSC 52 clipboard-set escape sequence.
//!
//! Writes the given text to the terminal's system clipboard via the
//! `ESC ] 52 ; c ; <base64> BEL` escape sequence. No OS-level clipboard API
//! is used, so this works through SSH and other remote sessions as long as
//! the host terminal supports OSC 52 (Windows Terminal does since 1.13).

use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use std::io::Write;

/// Sends the given text to the terminal's system clipboard.
///
/// Errors are silently swallowed — clipboard copy is best-effort UX, not a
/// critical operation. If stdout is closed or the terminal doesn't honor
/// OSC 52, the user simply doesn't get clipboard contents.
pub fn copy(text: &str) {
    let encoded = BASE64.encode(text);
    let seq = format!("\x1b]52;c;{}\x07", encoded);
    let mut stdout = std::io::stdout();
    let _ = stdout.write_all(seq.as_bytes());
    let _ = stdout.flush();
}
