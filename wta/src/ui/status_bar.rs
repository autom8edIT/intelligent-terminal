use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

use crate::app::{App, ConnectionState};
use crate::theme;

fn agent_identity(app: &App) -> String {
    let name = if app.agent_name.is_empty() {
        "agent"
    } else {
        &app.agent_name
    };

    match app.agent_model.as_deref() {
        Some(model) if !model.is_empty() => format!("{} {}", name, model),
        _ => name.to_string(),
    }
}

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    let identity = agent_identity(app);
    let identity_style = match &app.state {
        ConnectionState::Failed(_) => theme::STATUS_FAILED,
        _ if app.progress_status.is_some() || app.prompt_in_flight => theme::IN_PROGRESS,
        ConnectionState::Connected => theme::STATUS_CONNECTED,
        ConnectionState::Connecting(_) => theme::STATUS_CONNECTING,
        ConnectionState::Disconnected => theme::STATUS_DISCONNECTED,
    };

    let mut spans = vec![Span::styled(identity, identity_style)];
    if let Some(note) = app.timing_note.as_deref().filter(|note| !note.is_empty()) {
        spans.push(Span::styled(" | ", theme::DIM));
        spans.push(Span::styled(note.to_string(), theme::SYSTEM_TEXT));
    }

    let p = Paragraph::new(Line::from(spans));
    frame.render_widget(p, area);
}
