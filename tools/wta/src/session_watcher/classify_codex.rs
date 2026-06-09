//! Codex rollout classifier.
//!
//! Record shapes (verified 2026-06-08):
//!   * tool start: `{"type":"response_item","payload":{"type":"function_call","name":"shell_command"}}`
//!   * tool end:   `{"type":"response_item","payload":{"type":"function_call_output",...}}`
//!   * turn end:   `{"type":"event_msg","payload":{"type":"task_complete"}}`

use crate::agent_sessions::{is_user_input_tool, AgentKey, SessionEvent};

pub fn classify(record: &serde_json::Value, key: &AgentKey) -> Vec<SessionEvent> {
    let kind = record.get("type").and_then(|v| v.as_str()).unwrap_or("");
    let payload_type = record
        .get("payload")
        .and_then(|p| p.get("type"))
        .and_then(|v| v.as_str())
        .unwrap_or("");

    match (kind, payload_type) {
        ("response_item", "function_call")
        | ("response_item", "local_shell_call")
        | ("response_item", "custom_tool_call") => {
            let tool = record
                .get("payload")
                .and_then(|p| p.get("name"))
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();
            if is_user_input_tool(&tool) {
                vec![SessionEvent::Notification {
                    key: key.clone(),
                    message: tool,
                }]
            } else {
                vec![SessionEvent::ToolStarting {
                    key: key.clone(),
                    tool_name: tool,
                }]
            }
        }
        ("response_item", "function_call_output")
        | ("response_item", "custom_tool_call_output") => {
            vec![SessionEvent::ToolCompleted { key: key.clone() }]
        }
        ("event_msg", "task_complete") => vec![SessionEvent::SessionStopped {
            key: key.clone(),
            reason: "complete".to_string(),
        }],
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rec(s: &str) -> serde_json::Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn function_call_maps_to_tool_starting() {
        let r = rec(
            r#"{"type":"response_item","payload":{"type":"function_call","name":"shell_command"}}"#,
        );
        let out = classify(&r, &"k".to_string());
        assert_eq!(
            out,
            vec![SessionEvent::ToolStarting {
                key: "k".to_string(),
                tool_name: "shell_command".to_string()
            }]
        );
    }

    #[test]
    fn function_call_output_maps_to_tool_completed() {
        let r = rec(r#"{"type":"response_item","payload":{"type":"function_call_output"}}"#);
        assert_eq!(
            classify(&r, &"k".to_string()),
            vec![SessionEvent::ToolCompleted {
                key: "k".to_string()
            }]
        );
    }

    #[test]
    fn task_complete_maps_to_session_stopped() {
        let r = rec(r#"{"type":"event_msg","payload":{"type":"task_complete"}}"#);
        assert_eq!(
            classify(&r, &"k".to_string()),
            vec![SessionEvent::SessionStopped {
                key: "k".to_string(),
                reason: "complete".to_string()
            }]
        );
    }

    #[test]
    fn plain_message_yields_nothing() {
        let r = rec(r#"{"type":"response_item","payload":{"type":"message","role":"user"}}"#);
        assert!(classify(&r, &"k".to_string()).is_empty());
    }
}
