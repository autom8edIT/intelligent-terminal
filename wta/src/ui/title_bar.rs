use ratatui::prelude::*;
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, ConnectionState};
use crate::theme;

/// Two rows: title content, then a `─` separator vertically centered in the
/// row beneath it. The separator cell's natural top/bottom half-cell padding
/// gives the title some breathing room without an extra padding row.
pub const HEIGHT: u16 = 2;

pub fn render(frame: &mut Frame, app: &App, area: Rect) {
    if area.height == 0 || area.width == 0 {
        return;
    }

    // Row 0: title content. Row 1: separator.
    let title_area = Rect {
        x: area.x,
        y: area.y,
        width: area.width,
        height: 1,
    };

    // Paint the full bar with TITLE_BAR_BG so the title row and the bg
    // behind the separator glyph read as one contiguous band.
    frame.render_widget(
        Block::default().style(theme::TITLE_BAR_STYLE),
        area,
    );

    // ── [●] AgentName [version] [∨] ─────────────────────────────────────────
    let display_name = if app.agent_name.is_empty() {
        "Agent".to_string()
    } else {
        app.agent_name.clone()
    };

    let (dot, dot_style) = match &app.state {
        ConnectionState::Connected => ("●", theme::STATUS_CONNECTED),
        ConnectionState::Connecting(_) => ("●", theme::STATUS_CONNECTING),
        ConnectionState::Failed(_) => ("●", theme::STATUS_FAILED),
        ConnectionState::Disconnected => ("●", theme::STATUS_DISCONNECTED),
    };

    let label = if let Some(ver) = &app.agent_version {
        format!("{display_name} {ver}")
    } else if let Some(model) = &app.agent_model {
        if !model.is_empty() {
            format!("{display_name} {model}")
        } else {
            display_name
        }
    } else {
        display_name
    };

    let spans = vec![
        Span::raw(" "),
        Span::styled(dot, dot_style),
        Span::raw(" "),
        Span::styled(label, Style::new().fg(Color::White)),
        Span::styled(" ∨", theme::DIM),
    ];

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(theme::TITLE_BAR_STYLE),
        title_area,
    );

    // ── Row 1: separator ───────────────────────────────────────────────────
    // `─` is vertically centered in its cell, so the line sits in the
    // middle of row 1 with half a cell of black padding above and below it.
    if area.height >= HEIGHT {
        frame.render_widget(
            Paragraph::new("─".repeat(area.width as usize))
                .style(theme::TITLE_BAR_SEPARATOR),
            Rect {
                x: area.x,
                y: area.y + HEIGHT - 1,
                width: area.width,
                height: 1,
            },
        );
    }
}
