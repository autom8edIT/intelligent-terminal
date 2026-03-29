use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::backend::CrosstermBackend;
use ratatui::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::io;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;

use crate::coordinator::{
    parse_recommendation_set, recommended_choice_index, RecommendationChoice, RecommendationSet,
};
use crate::protocol::acp::client::{prompt_timing_log, PromptSubmission};
use crate::shared_host::SharedStateSnapshot;
use crate::ui;
use crate::ui_trace;

// --- Debug types ---

#[derive(Debug, Clone)]
pub enum DebugDir {
    Sent,
    Received,
}

#[derive(Debug, Clone)]
pub struct DebugMessage {
    pub timestamp: f64,
    pub direction: DebugDir,
    pub content: String,
}

// --- State types ---

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ConnectionState {
    Disconnected,
    Connecting(String),
    Connected,
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ChatMessage {
    User(String),
    Agent(String),
    System(String),
    ToolCall {
        id: String,
        title: String,
        status: String,
    },
    Plan(Vec<PlanEntry>),
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    pub status: PlanEntryStatus,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum PlanEntryStatus {
    Pending,
    InProgress,
    Completed,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PermOption {
    pub id: String,
    pub name: String,
    pub kind: String,
}

pub struct PermissionState {
    pub description: String,
    pub options: Vec<PermOption>,
    pub selected: usize,
    pub responder: Option<tokio::sync::oneshot::Sender<String>>,
}

// --- Events ---

pub enum AppEvent {
    Key(KeyEvent),
    Tick,
    Resize(u16, u16), // terminal resize (handled by ratatui)
    ConnectionStage(String),
    ProgressStatus(String),
    UserMessage(String),
    AgentConnected {
        name: String,
        model: Option<String>,
        session_id: String,
    },
    AgentError(String),
    AgentThoughtChunk(String),
    AgentMessageChunk(String),
    AgentMessageEnd,
    TimingMetric(String),
    ToolCall {
        id: String,
        title: String,
        status: String,
    },
    ToolCallUpdate {
        id: String,
        status: String,
    },
    Plan(Vec<PlanEntry>),
    PermissionRequest {
        description: String,
        options: Vec<PermOption>,
        responder: tokio::sync::oneshot::Sender<String>,
    },
    SharedPermissionRequest {
        description: String,
        options: Vec<PermOption>,
    },
    PermissionCleared,
    SystemMessage(String),
    DebugPipeMessage(DebugMessage),
    SharedStateSnapshot(SharedStateSnapshot),
}

// --- App ---

pub struct App {
    pub state: ConnectionState,
    pub agent_name: String,
    pub agent_model: Option<String>,
    pub progress_status: Option<String>,
    pub activity_frame: usize,
    pub session_id: String,
    pub wt_connected: bool,
    pub messages: Vec<ChatMessage>,
    pub input: String,
    pub cursor_pos: usize,
    pub tool_calls: HashMap<String, (String, String)>, // id -> (title, status)
    pub permission: Option<PermissionState>,
    pub scroll_offset: usize,
    pub agent_streaming: bool,
    pub recommendations: Option<RecommendationSet>,
    pub selected_recommendation: usize,
    pub should_quit: bool,
    pub prompt_in_flight: bool,
    pub shared_mode: bool,
    current_prompt_id: Option<u64>,
    current_prompt_submitted_at_unix_s: Option<f64>,
    selection_visible_pending: bool,
    prompt_tx: mpsc::UnboundedSender<PromptSubmission>,
    recommendation_tx: mpsc::UnboundedSender<RecommendationChoice>,
    permission_tx: mpsc::UnboundedSender<String>,
    pub pending_thought_response: String,
    pub pending_agent_response: String,
    pub timing_note: Option<String>,
    debug_capture_enabled: Arc<AtomicBool>,
    // Debug panel
    pub debug_messages: Vec<DebugMessage>,
    pub show_debug_panel: bool,
    pub debug_scroll: usize,
    // Pane identity (populated via VT channel)
    pub pane_id: Option<String>,
    pub tab_id: Option<String>,
    pub window_id: Option<String>,
}

impl App {
    pub fn new(
        prompt_tx: mpsc::UnboundedSender<PromptSubmission>,
        recommendation_tx: mpsc::UnboundedSender<RecommendationChoice>,
        permission_tx: mpsc::UnboundedSender<String>,
        debug_capture_enabled: Arc<AtomicBool>,
        wt_connected: bool,
        shared_mode: bool,
    ) -> Self {
        Self {
            state: ConnectionState::Connecting("Starting agent...".to_string()),
            agent_name: String::new(),
            agent_model: None,
            progress_status: None,
            activity_frame: 0,
            session_id: String::new(),
            wt_connected,
            messages: Vec::new(),
            input: String::new(),
            cursor_pos: 0,
            tool_calls: HashMap::new(),
            permission: None,
            scroll_offset: 0,
            agent_streaming: false,
            recommendations: None,
            selected_recommendation: 0,
            should_quit: false,
            prompt_in_flight: false,
            shared_mode,
            current_prompt_id: None,
            current_prompt_submitted_at_unix_s: None,
            selection_visible_pending: false,
            prompt_tx,
            recommendation_tx,
            permission_tx,
            pending_thought_response: String::new(),
            pending_agent_response: String::new(),
            timing_note: None,
            debug_capture_enabled,
            debug_messages: Vec::new(),
            show_debug_panel: false,
            debug_scroll: 0,
            pane_id: None,
            tab_id: None,
            window_id: None,
        }
    }

    pub async fn run(
        &mut self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        mut ui_rx: mpsc::UnboundedReceiver<AppEvent>,
        mut event_rx: mpsc::UnboundedReceiver<AppEvent>,
    ) -> Result<()> {
        const MAX_EVENTS_PER_FRAME: usize = 64;

        let initial_draw_started = std::time::Instant::now();
        self.draw_frame(terminal)?;
        ui_trace::log_slow("initial_draw", initial_draw_started.elapsed(), || {
            self.trace_state()
        });

        loop {
            tokio::select! {
                biased;

                Some(event) = ui_rx.recv() => {
                    let event_name = Self::event_name(&event);
                    self.apply_resize_if_needed(terminal, &event)?;
                    let should_redraw = self.event_requires_redraw(&event);
                    let handle_started = std::time::Instant::now();
                    self.handle_event(event);
                    ui_trace::log_slow("ui_event_handle", handle_started.elapsed(), || {
                        format!("event={} {}", event_name, self.trace_state())
                    });
                    if should_redraw {
                        let draw_started = std::time::Instant::now();
                        self.draw_frame(terminal)?;
                        ui_trace::log_slow("ui_event_draw", draw_started.elapsed(), || {
                            format!("event={} {}", event_name, self.trace_state())
                        });
                    }
                }

                Some(event) = event_rx.recv() => {
                    let first_event_name = Self::event_name(&event);
                    self.apply_resize_if_needed(terminal, &event)?;
                    let batch_started = std::time::Instant::now();
                    let mut processed = 0usize;

                    let mut should_redraw_now = self.event_requires_redraw(&event);
                    self.handle_event(event);
                    processed += 1;

                    while processed < MAX_EVENTS_PER_FRAME {
                        match event_rx.try_recv() {
                            Ok(event) => {
                                self.apply_resize_if_needed(terminal, &event)?;
                                if self.event_requires_redraw(&event) {
                                    should_redraw_now = true;
                                }
                                self.handle_event(event);
                                processed += 1;
                            }
                            Err(tokio::sync::mpsc::error::TryRecvError::Empty) => break,
                            Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => break,
                        }
                    }

                    ui_trace::log_slow("event_batch_handle", batch_started.elapsed(), || {
                        format!(
                            "first_event={} processed={} redraw={} {}",
                            first_event_name,
                            processed,
                            should_redraw_now,
                            self.trace_state()
                        )
                    });

                    if should_redraw_now {
                        let draw_started = std::time::Instant::now();
                        self.draw_frame(terminal)?;
                        ui_trace::log_slow("event_batch_draw", draw_started.elapsed(), || {
                            format!(
                                "first_event={} processed={} {}",
                                first_event_name,
                                processed,
                                self.trace_state()
                            )
                        });
                    }
                }

                else => {
                    break; // All senders dropped
                }
            }

            if self.should_quit {
                break;
            }
        }
        Ok(())
    }

    fn apply_resize_if_needed(
        &self,
        terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
        event: &AppEvent,
    ) -> Result<()> {
        let AppEvent::Resize(width, height) = event else {
            return Ok(());
        };

        let resize_started = std::time::Instant::now();
        terminal.resize(Rect::new(0, 0, *width, *height))?;
        ui_trace::log_slow("terminal_resize", resize_started.elapsed(), || {
            format!("width={} height={}", width, height)
        });
        Ok(())
    }

    fn draw_frame(&mut self, terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
        let total_started = std::time::Instant::now();

        let mut frame = terminal.get_frame();
        let area = frame.area();

        let render_started = std::time::Instant::now();
        ui::render(&mut frame, self);
        ui_trace::log_slow("ui_render", render_started.elapsed(), || self.trace_state());

        let flush_started = std::time::Instant::now();
        terminal.flush()?;
        ui_trace::log_slow("terminal_flush", flush_started.elapsed(), || {
            self.trace_state()
        });

        let cursor_started = std::time::Instant::now();
        match ui::input_cursor_position(self, area) {
            Some(position) => {
                terminal.show_cursor()?;
                terminal.set_cursor_position(position)?;
            }
            None => {
                terminal.hide_cursor()?;
            }
        }
        ui_trace::log_slow("terminal_cursor", cursor_started.elapsed(), || {
            self.trace_state()
        });

        terminal.swap_buffers();

        let backend_flush_started = std::time::Instant::now();
        terminal.backend_mut().flush()?;
        ui_trace::log_slow(
            "terminal_backend_flush",
            backend_flush_started.elapsed(),
            || self.trace_state(),
        );

        self.log_selection_visible_if_needed();

        ui_trace::log_slow("draw_frame_total", total_started.elapsed(), || {
            self.trace_state()
        });

        Ok(())
    }

    fn event_name(event: &AppEvent) -> &'static str {
        match event {
            AppEvent::Key(_) => "key",
            AppEvent::Tick => "tick",
            AppEvent::Resize(_, _) => "resize",
            AppEvent::ConnectionStage(_) => "connection_stage",
            AppEvent::ProgressStatus(_) => "progress_status",
            AppEvent::UserMessage(_) => "user_message",
            AppEvent::AgentConnected { .. } => "agent_connected",
            AppEvent::AgentError(_) => "agent_error",
            AppEvent::AgentThoughtChunk(_) => "agent_thought_chunk",
            AppEvent::AgentMessageChunk(_) => "agent_message_chunk",
            AppEvent::AgentMessageEnd => "agent_message_end",
            AppEvent::TimingMetric(_) => "timing_metric",
            AppEvent::ToolCall { .. } => "tool_call",
            AppEvent::ToolCallUpdate { .. } => "tool_call_update",
            AppEvent::Plan(_) => "plan",
            AppEvent::PermissionRequest { .. } => "permission_request",
            AppEvent::SharedPermissionRequest { .. } => "shared_permission_request",
            AppEvent::PermissionCleared => "permission_cleared",
            AppEvent::SystemMessage(_) => "system_message",
            AppEvent::DebugPipeMessage(_) => "debug_pipe_message",
            AppEvent::SharedStateSnapshot(_) => "shared_state_snapshot",
        }
    }

    fn trace_state(&self) -> String {
        format!(
            "state={:?} messages={} input_chars={} thought_chars={} pending_chars={} scroll={} streaming={} activity_frame={} recommendations={} permission={} timing_note={}",
            self.state,
            self.messages.len(),
            self.input.chars().count(),
            self.pending_thought_response.chars().count(),
            self.pending_agent_response.chars().count(),
            self.scroll_offset,
            self.agent_streaming,
            self.activity_frame,
            self.recommendations
                .as_ref()
                .map(|recs| recs.choices.len())
                .unwrap_or(0),
            self.permission.is_some(),
            self.timing_note.is_some()
        )
    }

    fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::Key(key) => self.handle_key(key),
            AppEvent::Tick => {
                if self.has_activity_indicator() {
                    self.activity_frame = (self.activity_frame + 1) % 9;
                }
            }
            AppEvent::Resize(_, _) => {} // ratatui handles resize
            AppEvent::ConnectionStage(stage) => {
                self.state = ConnectionState::Connecting(stage);
            }
            AppEvent::ProgressStatus(status) => {
                self.progress_status = Some(status);
                self.scroll_to_bottom();
            }
            AppEvent::UserMessage(text) => {
                self.prepare_for_new_prompt();
                self.messages.push(ChatMessage::User(text));
                self.scroll_to_bottom();
            }
            AppEvent::AgentConnected {
                name,
                model,
                session_id,
            } => {
                self.agent_name = name;
                self.agent_model = model;
                self.session_id = session_id;
                self.state = ConnectionState::Connected;
            }
            AppEvent::AgentError(msg) => {
                self.state = ConnectionState::Failed(msg.clone());
                self.prompt_in_flight = false;
                self.agent_streaming = false;
                self.progress_status = None;
                self.pending_thought_response.clear();
                self.activity_frame = 0;
                self.pending_agent_response.clear();
                self.timing_note = None;
                self.messages.push(ChatMessage::Error(msg));
            }
            AppEvent::AgentThoughtChunk(text) => {
                self.prompt_in_flight = true;
                if self.progress_status.is_none() {
                    self.progress_status = Some("Thinking...".to_string());
                }
                append_thought_preview(&mut self.pending_thought_response, &text);
                self.scroll_to_bottom();
            }
            AppEvent::AgentMessageChunk(text) => {
                self.agent_streaming = true;
                self.prompt_in_flight = true;
                self.progress_status = None;
                self.pending_thought_response.clear();
                self.pending_agent_response.push_str(&text);
                self.scroll_to_bottom();
            }
            AppEvent::AgentMessageEnd => {
                self.agent_streaming = false;
                self.prompt_in_flight = false;
                self.progress_status = None;
                self.pending_thought_response.clear();
                self.activity_frame = 0;
                let parsed_recommendations = self.finalize_agent_response();
                if parsed_recommendations {
                    self.clear_completed_turn_history();
                } else {
                    self.scroll_to_bottom();
                }
            }
            AppEvent::TimingMetric(note) => {
                self.timing_note = Some(note);
            }
            AppEvent::ToolCall { id, title, status } => {
                self.tool_calls
                    .insert(id.clone(), (title.clone(), status.clone()));
                self.messages
                    .push(ChatMessage::ToolCall { id, title, status });
                self.scroll_to_bottom();
            }
            AppEvent::ToolCallUpdate { id, status } => {
                if let Some(entry) = self.tool_calls.get_mut(&id) {
                    entry.1 = status.clone();
                }
                // Update in-place in messages
                for msg in &mut self.messages {
                    if let ChatMessage::ToolCall {
                        id: ref mid,
                        status: ref mut s,
                        ..
                    } = msg
                    {
                        if mid == &id {
                            *s = status.clone();
                        }
                    }
                }
            }
            AppEvent::Plan(entries) => {
                self.messages.push(ChatMessage::Plan(entries));
                self.scroll_to_bottom();
            }
            AppEvent::PermissionRequest {
                description,
                options,
                responder,
            } => {
                self.permission = Some(PermissionState {
                    description,
                    options,
                    selected: 0,
                    responder: Some(responder),
                });
            }
            AppEvent::SharedPermissionRequest {
                description,
                options,
            } => {
                self.permission = Some(PermissionState {
                    description,
                    options,
                    selected: 0,
                    responder: None,
                });
            }
            AppEvent::PermissionCleared => {
                self.permission = None;
            }
            AppEvent::SystemMessage(message) => {
                self.messages.push(ChatMessage::System(message));
                self.scroll_to_bottom();
            }
            AppEvent::DebugPipeMessage(msg) => {
                self.debug_messages.push(msg);
                // Cap at 500 messages
                if self.debug_messages.len() > 500 {
                    self.debug_messages.remove(0);
                }
            }
            AppEvent::SharedStateSnapshot(snapshot) => {
                self.apply_shared_snapshot(snapshot);
            }
        }
    }

    fn event_requires_redraw(&self, event: &AppEvent) -> bool {
        match event {
            AppEvent::Tick => self.has_activity_indicator(),
            AppEvent::AgentMessageChunk(_) => true,
            AppEvent::DebugPipeMessage(_) => self.show_debug_panel,
            _ => true,
        }
    }

    fn handle_key(&mut self, key: KeyEvent) {
        // If permission modal is showing, route keys there
        if let Some(ref mut perm) = self.permission {
            match key.code {
                KeyCode::Up => {
                    if perm.selected > 0 {
                        perm.selected -= 1;
                    }
                }
                KeyCode::Down => {
                    if perm.selected < perm.options.len().saturating_sub(1) {
                        perm.selected += 1;
                    }
                }
                KeyCode::Enter => {
                    let option_id = perm.options[perm.selected].id.clone();
                    // Take ownership to send
                    if let Some(perm) = self.permission.take() {
                        if let Some(responder) = perm.responder {
                            let _ = responder.send(option_id);
                        } else {
                            let _ = self.permission_tx.send(option_id);
                        }
                    }
                }
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    // Quick allow: find first allow option
                    if let Some(idx) = perm.options.iter().position(|o| o.kind.contains("allow")) {
                        let option_id = perm.options[idx].id.clone();
                        if let Some(perm) = self.permission.take() {
                            if let Some(responder) = perm.responder {
                                let _ = responder.send(option_id);
                            } else {
                                let _ = self.permission_tx.send(option_id);
                            }
                        }
                    }
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    // Quick deny: find first reject option
                    if let Some(idx) = perm.options.iter().position(|o| o.kind.contains("reject")) {
                        let option_id = perm.options[idx].id.clone();
                        if let Some(perm) = self.permission.take() {
                            if let Some(responder) = perm.responder {
                                let _ = responder.send(option_id);
                            } else {
                                let _ = self.permission_tx.send(option_id);
                            }
                        }
                    }
                }
                _ => {}
            }
            return;
        }

        match key.code {
            KeyCode::Up if self.input.is_empty() && self.recommendations.is_some() => {
                if self.selected_recommendation > 0 {
                    self.selected_recommendation -= 1;
                }
            }
            KeyCode::Down if self.input.is_empty() && self.recommendations.is_some() => {
                if let Some(recs) = &self.recommendations {
                    if self.selected_recommendation + 1 < recs.choices.len() {
                        self.selected_recommendation += 1;
                    }
                }
            }
            KeyCode::F(12) => {
                self.show_debug_panel = !self.show_debug_panel;
                self.debug_capture_enabled
                    .store(self.show_debug_panel, Ordering::Relaxed);
                self.debug_scroll = 0;
                return;
            }
            KeyCode::PageUp
                if key.modifiers.contains(KeyModifiers::SHIFT) && self.show_debug_panel =>
            {
                self.debug_scroll = self.debug_scroll.saturating_add(10);
                return;
            }
            KeyCode::PageDown
                if key.modifiers.contains(KeyModifiers::SHIFT) && self.show_debug_panel =>
            {
                self.debug_scroll = self.debug_scroll.saturating_sub(10);
                return;
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.agent_streaming {
                    // TODO: send cancel to agent
                    self.agent_streaming = false;
                } else {
                    self.should_quit = true;
                }
            }
            KeyCode::Enter => {
                if self.input.is_empty()
                    && self.state == ConnectionState::Connected
                    && self.recommendations.is_some()
                {
                    if let Some(choice) = self.selected_recommendation().cloned() {
                        self.clear_recommendations();
                        let _ = self.recommendation_tx.send(choice);
                    }
                } else if !self.input.is_empty() && self.state == ConnectionState::Connected {
                    let text = self.input.clone();
                    self.input.clear();
                    self.cursor_pos = 0;
                    if !self.shared_mode {
                        self.prepare_for_new_prompt();
                        self.messages.push(ChatMessage::User(text.clone()));
                        self.scroll_to_bottom();
                    }
                    let prompt = PromptSubmission::new(text, None);
                    self.current_prompt_id = Some(prompt.id);
                    self.current_prompt_submitted_at_unix_s = Some(prompt.submitted_at_unix_s);
                    self.selection_visible_pending = false;
                    prompt_timing_log(
                        prompt.id,
                        prompt.submitted_at_unix_s,
                        "ui_submit",
                        &format!("preview={:?}", prompt.preview()),
                    );
                    let _ = self.prompt_tx.send(prompt);
                }
            }
            KeyCode::Backspace => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Delete => {
                if self.cursor_pos < self.input.len() {
                    self.input.remove(self.cursor_pos);
                }
            }
            KeyCode::Left => {
                if self.cursor_pos > 0 {
                    self.cursor_pos -= 1;
                }
            }
            KeyCode::Right => {
                if self.cursor_pos < self.input.len() {
                    self.cursor_pos += 1;
                }
            }
            KeyCode::Home => {
                self.cursor_pos = 0;
            }
            KeyCode::End => {
                self.cursor_pos = self.input.len();
            }
            KeyCode::PageUp => {
                self.scroll_offset = self.scroll_offset.saturating_add(10);
            }
            KeyCode::PageDown => {
                self.scroll_offset = self.scroll_offset.saturating_sub(10);
            }
            KeyCode::Char(c) => {
                self.input.insert(self.cursor_pos, c);
                self.cursor_pos += 1;
            }
            _ => {}
        }
    }

    fn scroll_to_bottom(&mut self) {
        self.scroll_offset = 0;
    }

    fn has_activity_indicator(&self) -> bool {
        self.prompt_in_flight || self.agent_streaming || self.progress_status.is_some()
    }

    fn clear_recommendations(&mut self) {
        self.recommendations = None;
        self.selected_recommendation = 0;
    }

    fn clear_chat_history(&mut self) {
        self.messages.clear();
        self.tool_calls.clear();
        self.permission = None;
        self.progress_status = None;
        self.pending_thought_response.clear();
        self.activity_frame = 0;
        self.pending_agent_response.clear();
        self.agent_streaming = false;
        self.scroll_offset = 0;
        self.timing_note = None;
        self.selection_visible_pending = false;
        self.clear_recommendations();
    }

    fn clear_completed_turn_history(&mut self) {
        self.messages.clear();
        self.tool_calls.clear();
        self.permission = None;
        self.progress_status = None;
        self.pending_thought_response.clear();
        self.activity_frame = 0;
        self.pending_agent_response.clear();
        self.agent_streaming = false;
        self.scroll_offset = 0;
        self.selection_visible_pending = false;
    }

    fn prepare_for_new_prompt(&mut self) {
        self.clear_chat_history();
        self.prompt_in_flight = true;
        self.progress_status = Some("Preparing context...".to_string());
        self.activity_frame = 0;
    }

    fn selected_recommendation(&self) -> Option<&RecommendationChoice> {
        self.recommendations
            .as_ref()
            .and_then(|recs| recs.choices.get(self.selected_recommendation))
    }

    fn finalize_agent_response(&mut self) -> bool {
        if self.pending_agent_response.trim().is_empty() {
            self.log_selection_phase("selection_parse_failed", "reason=empty_agent_response");
            return false;
        }

        let text = std::mem::take(&mut self.pending_agent_response);

        match parse_recommendation_set(&text) {
            Ok(recommendations) => {
                self.selected_recommendation = recommended_choice_index(&recommendations);
                self.log_selection_phase(
                    "selection_ready",
                    &format!(
                        "choice_count={} recommended_choice={:?}",
                        recommendations.choices.len(),
                        recommendations.recommended_choice
                    ),
                );
                self.recommendations = Some(recommendations);
                self.selection_visible_pending = true;
                true
            }
            Err(err) => {
                self.clear_recommendations();
                let error_text = format!("{:#}", err).replace('\n', " | ");
                self.log_selection_phase(
                    "selection_parse_failed",
                    &format!(
                        "response_chars={} error={:?}",
                        text.chars().count(),
                        error_text
                    ),
                );
                self.messages.push(ChatMessage::System(format!(
                    "Agent returned invalid recommendation JSON: {}",
                    error_text
                )));
                self.messages.push(ChatMessage::Agent(text));
                false
            }
        }
    }

    fn apply_shared_snapshot(&mut self, snapshot: SharedStateSnapshot) {
        let recommendations_changed = self.recommendations != snapshot.recommendations;
        let permission_changed = self
            .permission
            .as_ref()
            .map(|perm| (&perm.description, &perm.options))
            != snapshot
                .permission
                .as_ref()
                .map(|perm| (&perm.description, &perm.options));

        self.state = snapshot.state;
        self.agent_name = snapshot.agent_name;
        self.agent_model = snapshot.agent_model;
        self.progress_status = snapshot.progress_status;
        self.session_id = snapshot.session_id;
        self.wt_connected = snapshot.wt_connected;
        self.messages = snapshot.messages;
        self.recommendations = snapshot.recommendations;
        self.agent_streaming = snapshot.agent_streaming;
        self.pending_thought_response = snapshot.pending_thought_response;
        self.pending_agent_response = snapshot.pending_agent_response;
        self.timing_note = snapshot.timing_note;
        self.prompt_in_flight = snapshot.prompt_in_flight;

        if recommendations_changed {
            self.selected_recommendation = self
                .recommendations
                .as_ref()
                .map(recommended_choice_index)
                .unwrap_or(0);
            if self.recommendations.is_some() {
                self.selection_visible_pending = true;
            }
        }

        if let Some(permission) = snapshot.permission {
            let selected = if permission_changed {
                0
            } else {
                self.permission
                    .as_ref()
                    .map(|current| current.selected)
                    .unwrap_or(0)
            };
            let max_selected = permission.options.len().saturating_sub(1);
            self.permission = Some(PermissionState {
                description: permission.description,
                options: permission.options,
                selected: selected.min(max_selected),
                responder: None,
            });
        } else {
            self.permission = None;
        }
    }
}

impl App {
    fn log_selection_phase(&self, phase: &str, details: &str) {
        if let (Some(prompt_id), Some(submitted_at_unix_s)) = (
            self.current_prompt_id,
            self.current_prompt_submitted_at_unix_s,
        ) {
            prompt_timing_log(prompt_id, submitted_at_unix_s, phase, details);
        }
    }

    fn log_selection_visible_if_needed(&mut self) {
        if !self.selection_visible_pending || self.recommendations.is_none() {
            return;
        }

        let details = format!(
            "choice_count={} selected_index={}",
            self.recommendations
                .as_ref()
                .map(|set| set.choices.len())
                .unwrap_or(0),
            self.selected_recommendation
        );
        self.log_selection_phase("selection_visible", &details);
        self.selection_visible_pending = false;
    }
}

const THOUGHT_PREVIEW_MAX_CHARS: usize = 1024;

fn append_thought_preview(buffer: &mut String, chunk: &str) {
    if chunk.is_empty() {
        return;
    }

    buffer.push_str(chunk);
    let char_count = buffer.chars().count();
    if char_count <= THOUGHT_PREVIEW_MAX_CHARS {
        return;
    }

    let tail: String = buffer
        .chars()
        .skip(char_count.saturating_sub(THOUGHT_PREVIEW_MAX_CHARS))
        .collect();
    *buffer = format!("...{tail}");
}
