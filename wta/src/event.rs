use crossterm::event::{Event, EventStream};
use futures::StreamExt;
use tokio::sync::mpsc;
use tokio::time::{self, Duration, MissedTickBehavior};

use crate::app::AppEvent;

pub async fn read_crossterm_events(tx: mpsc::UnboundedSender<AppEvent>) {
    let mut reader = EventStream::new();
    let mut ticker = time::interval(Duration::from_millis(120));
    ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            _ = ticker.tick() => {
                if tx.send(AppEvent::Tick).is_err() {
                    break;
                }
            }
            maybe_event = reader.next() => {
                let Some(Ok(event)) = maybe_event else {
                    break;
                };
                let app_event = match event {
                    Event::Key(key) if key.kind == crossterm::event::KeyEventKind::Press => {
                        AppEvent::Key(key)
                    }
                    Event::Resize(w, h) => AppEvent::Resize(w, h),
                    _ => continue,
                };
                if tx.send(app_event).is_err() {
                    break;
                }
            }
        }
    }
}
