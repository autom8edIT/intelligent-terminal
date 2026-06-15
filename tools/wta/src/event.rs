use crossterm::event::{Event, EventStream, KeyCode, KeyEvent, KeyModifiers};
use futures::StreamExt;
use std::time::Instant;
use tokio::sync::mpsc;
use tokio::time::{self, Duration, MissedTickBehavior};

use crate::app::AppEvent;

/// Maximum wait between bytes of a single VT escape sequence. Real
/// sequences arrive sub-millisecond; user keystrokes (Esc, then later
/// typing) are tens of ms apart. 30ms cleanly disambiguates without
/// adding perceptible latency to a bare Esc press.
const VT_TIMEOUT: Duration = Duration::from_millis(30);

/// VT escape-sequence collector. conpty in this build delivers CSI/SS3
/// sequences as separate `Esc` + `Char('[')`/`Char('O')` + parameter
/// chars + final byte events instead of pre-parsed `KeyCode::Up`/etc.
/// This state machine reassembles them at the event boundary.
///
/// Safety property: unknown CSI / SS3 finals are **dropped silently**.
/// Inserting raw bytes would either trigger the Esc cascade (clear
/// input / cancel turn) or pollute the input box with junk like `[15~`.
#[derive(Debug)]
enum VtState {
    Idle,
    Esc { since: Instant },
    Csi { since: Instant, params: Vec<u16>, current: Option<u16> },
    Ss3 { since: Instant },
}

impl VtState {
    fn pending_since(&self) -> Option<Instant> {
        match self {
            VtState::Idle => None,
            VtState::Esc { since }
            | VtState::Csi { since, .. }
            | VtState::Ss3 { since } => Some(*since),
        }
    }
}

fn decode_csi(params: &[u16], final_byte: char) -> Option<KeyCode> {
    match final_byte {
        'A' => Some(KeyCode::Up),
        'B' => Some(KeyCode::Down),
        'C' => Some(KeyCode::Right),
        'D' => Some(KeyCode::Left),
        'H' => Some(KeyCode::Home),
        'F' => Some(KeyCode::End),
        '~' => match params.first().copied() {
            Some(1) | Some(7) => Some(KeyCode::Home),
            Some(2) => Some(KeyCode::Insert),
            Some(3) => Some(KeyCode::Delete),
            Some(4) | Some(8) => Some(KeyCode::End),
            Some(5) => Some(KeyCode::PageUp),
            Some(6) => Some(KeyCode::PageDown),
            Some(15) => Some(KeyCode::F(5)),
            Some(17) => Some(KeyCode::F(6)),
            Some(18) => Some(KeyCode::F(7)),
            Some(19) => Some(KeyCode::F(8)),
            Some(20) => Some(KeyCode::F(9)),
            Some(21) => Some(KeyCode::F(10)),
            Some(23) => Some(KeyCode::F(11)),
            Some(24) => Some(KeyCode::F(12)),
            _ => None,
        },
        _ => None,
    }
}

fn decode_ss3(final_byte: char) -> Option<KeyCode> {
    match final_byte {
        'A' => Some(KeyCode::Up),
        'B' => Some(KeyCode::Down),
        'C' => Some(KeyCode::Right),
        'D' => Some(KeyCode::Left),
        'H' => Some(KeyCode::Home),
        'F' => Some(KeyCode::End),
        'P' => Some(KeyCode::F(1)),
        'Q' => Some(KeyCode::F(2)),
        'R' => Some(KeyCode::F(3)),
        'S' => Some(KeyCode::F(4)),
        _ => None,
    }
}

fn make_key(code: KeyCode) -> KeyEvent {
    KeyEvent::new(code, KeyModifiers::NONE)
}

/// Drive one key through the state machine; returns events to emit.
fn vt_step(state: &mut VtState, key: KeyEvent) -> Vec<KeyEvent> {
    let plain = key.modifiers.is_empty();
    let mut out = Vec::new();
    match state {
        VtState::Idle => match (key.code, plain) {
            (KeyCode::Esc, true) => {
                *state = VtState::Esc { since: Instant::now() };
            }
            _ => out.push(key),
        },
        VtState::Esc { .. } => match (key.code, plain) {
            (KeyCode::Char('['), true) => {
                *state = VtState::Csi { since: Instant::now(), params: Vec::new(), current: None };
            }
            (KeyCode::Char('O'), true) => {
                *state = VtState::Ss3 { since: Instant::now() };
            }
            _ => {
                *state = VtState::Idle;
                out.push(make_key(KeyCode::Esc));
                out.push(key);
            }
        },
        VtState::Csi { params, current, .. } => match (key.code, plain) {
            (KeyCode::Char(c), true) if c.is_ascii_digit() => {
                let digit = (c as u16) - ('0' as u16);
                *current = Some(current.unwrap_or(0).saturating_mul(10).saturating_add(digit));
            }
            (KeyCode::Char(';'), true) => {
                params.push(current.take().unwrap_or(0));
            }
            (KeyCode::Char(c), true)
                if c.is_ascii_alphabetic() || c == '~' || c == '@' =>
            {
                if let Some(n) = current.take() {
                    params.push(n);
                }
                let decoded = decode_csi(params, c);
                *state = VtState::Idle;
                if let Some(kc) = decoded {
                    out.push(make_key(kc));
                }
            }
            _ => {
                *state = VtState::Idle;
                out.extend(vt_step(state, key));
            }
        },
        VtState::Ss3 { .. } => match (key.code, plain) {
            (KeyCode::Char(c), true) if c.is_ascii_alphabetic() => {
                let decoded = decode_ss3(c);
                *state = VtState::Idle;
                if let Some(kc) = decoded {
                    out.push(make_key(kc));
                }
            }
            _ => {
                *state = VtState::Idle;
                out.extend(vt_step(state, key));
            }
        },
    }
    out
}

fn vt_flush_timeout(state: &mut VtState) -> Vec<KeyEvent> {
    let mut out = Vec::new();
    match std::mem::replace(state, VtState::Idle) {
        VtState::Esc { .. } => out.push(make_key(KeyCode::Esc)),
        VtState::Csi { .. } | VtState::Ss3 { .. } | VtState::Idle => {}
    }
    out
}

pub async fn read_crossterm_events(tx: mpsc::UnboundedSender<AppEvent>) {
    let mut reader = EventStream::new();
    let mut ticker = time::interval(Duration::from_millis(120));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    let mut reveal_ticker = time::interval(Duration::from_millis(33));
    reveal_ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    tracing::info!(target: "input", "crossterm reader task starting");
    let mut consecutive_errors = 0usize;
    let mut vt = VtState::Idle;

    let send_key = |tx: &mpsc::UnboundedSender<AppEvent>, key: KeyEvent| -> bool {
        tracing::trace!(
            target: "input",
            code = ?key.code,
            mods = ?key.modifiers,
            "key dispatched",
        );
        tx.send(AppEvent::Key(key)).is_ok()
    };

    loop {
        let vt_deadline = vt.pending_since().map(|since| since + VT_TIMEOUT);

        tokio::select! {
            _ = ticker.tick() => {
                if tx.send(AppEvent::Tick).is_err() {
                    tracing::info!(target: "input", "crossterm reader exiting: AppEvent channel closed");
                    break;
                }
            }
            _ = reveal_ticker.tick() => {
                if tx.send(AppEvent::RevealTick).is_err() {
                    tracing::info!(target: "input", "crossterm reader exiting: AppEvent channel closed");
                    break;
                }
            }
            _ = async {
                if let Some(deadline) = vt_deadline {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(deadline)).await;
                } else {
                    std::future::pending::<()>().await;
                }
            }, if vt_deadline.is_some() => {
                for key in vt_flush_timeout(&mut vt) {
                    if !send_key(&tx, key) { break; }
                }
            }
            maybe_event = reader.next() => {
                let event = match maybe_event {
                    Some(Ok(e)) => {
                        consecutive_errors = 0;
                        e
                    }
                    Some(Err(e)) => {
                        consecutive_errors += 1;
                        tracing::warn!(
                            target: "input",
                            error = %e,
                            consecutive = consecutive_errors,
                            "crossterm read error, continuing",
                        );
                        if consecutive_errors >= 8 {
                            tracing::warn!(target: "input", "rebuilding EventStream after sustained read errors");
                            reader = EventStream::new();
                            consecutive_errors = 0;
                        }
                        continue;
                    }
                    None => {
                        tracing::info!(target: "input", "crossterm reader EOF, exiting");
                        break;
                    }
                };

                // Normalize conpty's raw VT bytes at the event boundary:
                //   1. Backspace as `Char('\u{7f}')` → `KeyCode::Backspace`
                //      (also Ctrl+H 0x08 from legacy emulators).
                //   2. Arrow keys / F-keys / page-nav as `Esc [ ... letter`
                //      or `Esc O letter` → assembled into the right
                //      `KeyCode` by the VT state machine. Unknown
                //      sequences drop silently — never insert raw bytes
                //      that would either trigger the Esc cascade or
                //      pollute the input box with junk like "[15~".
                if let Event::Key(mut key) = event {
                    if key.kind != crossterm::event::KeyEventKind::Press {
                        continue;
                    }
                    match (key.code, key.modifiers) {
                        (KeyCode::Char('\u{7f}'), _) | (KeyCode::Char('\u{8}'), _) => {
                            key.code = KeyCode::Backspace;
                        }
                        _ => {}
                    }
                    for out_key in vt_step(&mut vt, key) {
                        if !send_key(&tx, out_key) { return; }
                    }
                } else {
                    for key in vt_flush_timeout(&mut vt) {
                        if !send_key(&tx, key) { break; }
                    }
                    let app_event = match event {
                        Event::Resize(w, h) => AppEvent::Resize(w, h),
                        Event::FocusGained => AppEvent::FocusChanged(true),
                        Event::FocusLost => AppEvent::FocusChanged(false),
                        _ => continue,
                    };
                    if tx.send(app_event).is_err() {
                        tracing::info!(target: "input", "crossterm reader exiting: AppEvent channel closed");
                        break;
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn k(code: KeyCode) -> KeyEvent {
        KeyEvent::new(code, KeyModifiers::NONE)
    }

    #[test]
    fn decode_csi_simple_finals() {
        assert_eq!(decode_csi(&[], 'A'), Some(KeyCode::Up));
        assert_eq!(decode_csi(&[], 'B'), Some(KeyCode::Down));
        assert_eq!(decode_csi(&[], 'C'), Some(KeyCode::Right));
        assert_eq!(decode_csi(&[], 'D'), Some(KeyCode::Left));
        assert_eq!(decode_csi(&[], 'H'), Some(KeyCode::Home));
        assert_eq!(decode_csi(&[], 'F'), Some(KeyCode::End));
    }

    #[test]
    fn decode_csi_tilde_navigation_keys() {
        assert_eq!(decode_csi(&[2], '~'), Some(KeyCode::Insert));
        assert_eq!(decode_csi(&[3], '~'), Some(KeyCode::Delete));
        assert_eq!(decode_csi(&[5], '~'), Some(KeyCode::PageUp));
        assert_eq!(decode_csi(&[6], '~'), Some(KeyCode::PageDown));
        assert_eq!(decode_csi(&[1], '~'), Some(KeyCode::Home));
        assert_eq!(decode_csi(&[4], '~'), Some(KeyCode::End));
    }

    #[test]
    fn decode_csi_function_keys_f5_through_f12() {
        for (param, n) in [(15, 5), (17, 6), (18, 7), (19, 8), (20, 9), (21, 10), (23, 11), (24, 12)] {
            assert_eq!(decode_csi(&[param], '~'), Some(KeyCode::F(n)));
        }
    }

    #[test]
    fn decode_csi_unknown_returns_none() {
        assert!(decode_csi(&[], 'Z').is_none());
        assert!(decode_csi(&[99], '~').is_none());
    }

    #[test]
    fn decode_ss3_function_and_arrow_keys() {
        assert_eq!(decode_ss3('P'), Some(KeyCode::F(1)));
        assert_eq!(decode_ss3('Q'), Some(KeyCode::F(2)));
        assert_eq!(decode_ss3('A'), Some(KeyCode::Up));
    }

    fn drive(seq: &[KeyEvent]) -> Vec<KeyEvent> {
        let mut s = VtState::Idle;
        let mut out = Vec::new();
        for k in seq {
            out.extend(vt_step(&mut s, *k));
        }
        out.extend(vt_flush_timeout(&mut s));
        out
    }

    #[test]
    fn vt_decodes_arrow_keys() {
        for (final_ch, code) in [('A', KeyCode::Up), ('B', KeyCode::Down), ('C', KeyCode::Right), ('D', KeyCode::Left)] {
            let out = drive(&[k(KeyCode::Esc), k(KeyCode::Char('[')), k(KeyCode::Char(final_ch))]);
            assert_eq!(out, vec![k(code)], "Esc [ {final_ch} must decode to {code:?}");
        }
    }

    #[test]
    fn vt_decodes_delete_pageup_pagedown() {
        assert_eq!(drive(&[k(KeyCode::Esc), k(KeyCode::Char('[')), k(KeyCode::Char('3')), k(KeyCode::Char('~'))]),
            vec![k(KeyCode::Delete)]);
        assert_eq!(drive(&[k(KeyCode::Esc), k(KeyCode::Char('[')), k(KeyCode::Char('5')), k(KeyCode::Char('~'))]),
            vec![k(KeyCode::PageUp)]);
        assert_eq!(drive(&[k(KeyCode::Esc), k(KeyCode::Char('[')), k(KeyCode::Char('6')), k(KeyCode::Char('~'))]),
            vec![k(KeyCode::PageDown)]);
    }

    #[test]
    fn vt_decodes_multi_digit_f12() {
        assert_eq!(drive(&[k(KeyCode::Esc), k(KeyCode::Char('[')), k(KeyCode::Char('2')), k(KeyCode::Char('4')), k(KeyCode::Char('~'))]),
            vec![k(KeyCode::F(12))]);
    }

    #[test]
    fn vt_decodes_ss3_function_keys() {
        assert_eq!(drive(&[k(KeyCode::Esc), k(KeyCode::Char('O')), k(KeyCode::Char('P'))]),
            vec![k(KeyCode::F(1))]);
    }

    #[test]
    fn vt_bare_esc_flushes_on_timeout() {
        let mut s = VtState::Idle;
        assert!(vt_step(&mut s, k(KeyCode::Esc)).is_empty());
        assert_eq!(vt_flush_timeout(&mut s), vec![k(KeyCode::Esc)]);
    }

    #[test]
    fn vt_unknown_csi_drops_silently() {
        // Safety property: unknown CSI finals must NOT flush as raw
        // chars. Otherwise pressing F-keys or unknown sequences would
        // (a) trigger the Esc cascade (clear input / cancel turn) and
        // (b) insert garbage like "[15~" into the input box.
        let out = drive(&[k(KeyCode::Esc), k(KeyCode::Char('[')), k(KeyCode::Char('Z'))]);
        assert!(out.is_empty(), "unknown CSI must drop silently; got {out:?}");
    }

    #[test]
    fn vt_unknown_ss3_drops_silently() {
        let out = drive(&[k(KeyCode::Esc), k(KeyCode::Char('O')), k(KeyCode::Char('Z'))]);
        assert!(out.is_empty(), "unknown SS3 must drop silently; got {out:?}");
    }

    #[test]
    fn vt_partial_csi_timeout_drops_silently() {
        let mut s = VtState::Idle;
        vt_step(&mut s, k(KeyCode::Esc));
        vt_step(&mut s, k(KeyCode::Char('[')));
        let out = vt_flush_timeout(&mut s);
        assert!(out.is_empty(), "partial CSI timeout must drop silently; got {out:?}");
    }

    #[test]
    fn vt_esc_then_regular_char_flushes_both() {
        let out = drive(&[k(KeyCode::Esc), k(KeyCode::Char('x'))]);
        assert_eq!(out, vec![k(KeyCode::Esc), k(KeyCode::Char('x'))]);
    }

    #[test]
    fn vt_esc_esc_flushes_both() {
        let out = drive(&[k(KeyCode::Esc), k(KeyCode::Esc)]);
        assert_eq!(out, vec![k(KeyCode::Esc), k(KeyCode::Esc)]);
    }

    #[test]
    fn vt_plain_typing_passes_through() {
        let out = drive(&[k(KeyCode::Char('h')), k(KeyCode::Char('i')), k(KeyCode::Char('!'))]);
        assert_eq!(out, vec![k(KeyCode::Char('h')), k(KeyCode::Char('i')), k(KeyCode::Char('!'))]);
    }

    #[test]
    fn vt_back_to_back_arrows() {
        let mut s = VtState::Idle;
        let mut all = Vec::new();
        for _ in 0..3 {
            all.extend(vt_step(&mut s, k(KeyCode::Esc)));
            all.extend(vt_step(&mut s, k(KeyCode::Char('['))));
            all.extend(vt_step(&mut s, k(KeyCode::Char('D'))));
        }
        assert_eq!(all, vec![k(KeyCode::Left); 3]);
    }

    #[test]
    fn vt_ctrl_modifier_passes_through_without_entering_parser() {
        // Ctrl+C must not enter the parser. The plain-modifier guard
        // catches it.
        let out = drive(&[KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)]);
        assert_eq!(out, vec![KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)]);
    }

    #[test]
    fn vt_modified_arrow_falls_back_to_plain_arrow() {
        // CSI 1 ; 5 D = Ctrl+Left. We don't decode the modifier param;
        // emit plain Left (cursor still moves, modifier silently lost).
        // Acceptable degradation vs. inserting "[1;5D" into the input.
        let out = drive(&[
            k(KeyCode::Esc), k(KeyCode::Char('[')),
            k(KeyCode::Char('1')), k(KeyCode::Char(';')), k(KeyCode::Char('5')),
            k(KeyCode::Char('D')),
        ]);
        assert_eq!(out, vec![k(KeyCode::Left)]);
    }
}
