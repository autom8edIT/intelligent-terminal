use std::collections::BTreeSet;
use std::sync::Arc;

use anyhow::{anyhow, bail, Context, Result};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;
use tokio::time::{sleep, Duration};

use crate::app::AppEvent;
use crate::shell::ShellManager;

#[derive(Debug, Clone, Serialize)]
pub struct SupportedDelegateAgent {
    pub id: String,
    pub name: String,
    pub description: String,
}

#[derive(Debug, Clone)]
pub struct DelegateAgentRuntime {
    pub id: String,
    pub name: String,
    pub description: String,
    pub command: String,
    pub model: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendationSet {
    #[serde(default)]
    pub recommended_choice: Option<usize>,
    pub choices: Vec<RecommendationChoice>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecommendationChoice {
    pub choice: usize,
    pub title: String,
    #[serde(default)]
    pub rationale: String,
    pub actions: Vec<RecommendedAction>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RecommendedAction {
    RunCommand {
        parent: String,
        command: String,
    },
    SendPrompt {
        parent: String,
        prompt: String,
    },
    CreateShellTab {
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        commandline: Option<String>,
    },
    CreateShellPanel {
        parent: String,
        #[serde(default)]
        direction: Option<String>,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        commandline: Option<String>,
    },
    DelegateTab {
        agent: String,
        prompt: String,
        #[serde(default)]
        cwd: Option<String>,
        #[serde(default)]
        title: Option<String>,
    },
}

pub fn default_supported_delegate_agents() -> Vec<SupportedDelegateAgent> {
    vec![SupportedDelegateAgent {
        id: "copilot".to_string(),
        name: "GitHub Copilot".to_string(),
        description:
            "Launches `copilot` in a new terminal target with a self-contained startup task prompt."
                .to_string(),
    }]
}

pub fn default_delegate_agent_runtimes() -> Vec<DelegateAgentRuntime> {
    vec![DelegateAgentRuntime {
        id: "copilot".to_string(),
        name: "GitHub Copilot".to_string(),
        description:
            "Launches `copilot` directly in a new terminal target with an interactive startup task prompt."
                .to_string(),
        command: "copilot".to_string(),
        model: None,
    }]
}

pub fn parse_recommendation_set(text: &str) -> Result<RecommendationSet> {
    let json = extract_json_code_block(text)
        .or_else(|| extract_first_json_object(text))
        .context("no recommendation JSON block found")?;

    let mut parsed: RecommendationSet =
        serde_json::from_str(json).context("failed to parse recommendation JSON")?;
    validate_recommendation_set(&parsed)?;
    parsed.choices.sort_by_key(|c| c.choice);
    Ok(parsed)
}

pub fn recommended_choice_index(set: &RecommendationSet) -> usize {
    if let Some(choice_no) = set.recommended_choice {
        if let Some(idx) = set
            .choices
            .iter()
            .position(|choice| choice.choice == choice_no)
        {
            return idx;
        }
    }
    0
}

pub async fn run_recommendation_executor(
    mut rx: mpsc::UnboundedReceiver<RecommendationChoice>,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    shell_mgr: Arc<ShellManager>,
    delegate_agents: Vec<DelegateAgentRuntime>,
) {
    while let Some(choice) = rx.recv().await {
        match execute_choice(&choice, &shell_mgr, &delegate_agents, &event_tx).await {
            Ok(()) => {}
            Err(err) => {
                let _ = event_tx.send(AppEvent::SystemMessage(format!(
                    "Choice {} failed: {:#}",
                    choice.choice, err
                )));
            }
        }
    }
}

async fn execute_choice(
    choice: &RecommendationChoice,
    shell_mgr: &ShellManager,
    delegate_agents: &[DelegateAgentRuntime],
    event_tx: &mpsc::UnboundedSender<AppEvent>,
) -> Result<()> {
    for action in &choice.actions {
        match action {
            RecommendedAction::RunCommand { parent, command } => {
                ensure_non_empty("parent", parent)?;
                ensure_non_empty("command", command)?;
                let payload = format!("{command}\r");
                shell_mgr
                    .wt_send_input(parent, &payload)
                    .await
                    .with_context(|| format!("failed to send command to pane {}", parent))?;
                let _ = event_tx.send(AppEvent::SystemMessage(format!(
                    "Sent command to pane {}.",
                    parent
                )));
            }
            RecommendedAction::SendPrompt { parent, prompt } => {
                ensure_non_empty("parent", parent)?;
                ensure_non_empty("prompt", prompt)?;
                let payload = format!("{prompt}\r");
                shell_mgr
                    .wt_send_input(parent, &payload)
                    .await
                    .with_context(|| format!("failed to send prompt to pane {}", parent))?;
                let _ = event_tx.send(AppEvent::SystemMessage(format!(
                    "Sent prompt to pane {}.",
                    parent
                )));
            }
            RecommendedAction::CreateShellTab {
                title,
                cwd,
                commandline,
            } => {
                let result = shell_mgr
                    .wt_create_tab(None, cwd.as_deref(), title.as_deref())
                    .await
                    .context("failed to create shell tab")?;
                let pane_id =
                    value_to_string(result.get("pane_id")).unwrap_or_else(|| "?".to_string());
                if let Some(command) = non_empty_text(commandline.as_deref()) {
                    send_shell_command_to_new_pane(shell_mgr, &pane_id, command).await?;
                }
            }
            RecommendedAction::CreateShellPanel {
                parent,
                direction,
                title: _,
                cwd,
                commandline,
            } => {
                ensure_non_empty("parent", parent)?;
                let result = shell_mgr
                    .wt_split_pane(
                        parent,
                        None,
                        cwd.as_deref(),
                        normalize_direction(direction.as_deref())?,
                        None,
                    )
                    .await
                    .with_context(|| format!("failed to split pane {}", parent))?;
                let pane_id =
                    value_to_string(result.get("pane_id")).unwrap_or_else(|| "?".to_string());
                if let Some(command) = non_empty_text(commandline.as_deref()) {
                    send_shell_command_to_new_pane(shell_mgr, &pane_id, command).await?;
                }
            }
            RecommendedAction::DelegateTab {
                agent,
                prompt,
                cwd,
                title,
            } => {
                let runtime = lookup_delegate_agent(delegate_agents, agent)?;
                let commandline = build_delegate_commandline(runtime, prompt)?;
                let result = shell_mgr
                    .wt_create_tab(
                        Some(&commandline),
                        cwd.as_deref(),
                        title.as_deref().or(Some(runtime.name.as_str())),
                    )
                    .await
                    .with_context(|| {
                        format!("failed to create delegate tab for {}", runtime.name)
                    })?;
                let _pane_id =
                    value_to_string(result.get("pane_id")).unwrap_or_else(|| "?".to_string());
            }
        }
    }

    Ok(())
}

fn validate_recommendation_set(set: &RecommendationSet) -> Result<()> {
    if set.choices.len() != 3 {
        bail!("expected exactly 3 choices, got {}", set.choices.len());
    }

    let mut seen = BTreeSet::new();
    for choice in &set.choices {
        if !(1..=3).contains(&choice.choice) {
            bail!("choice numbers must be 1..=3");
        }
        if !seen.insert(choice.choice) {
            bail!("duplicate choice number {}", choice.choice);
        }
        ensure_non_empty("title", &choice.title)?;
        if choice.actions.is_empty() {
            bail!("choice {} has no actions", choice.choice);
        }
        for action in &choice.actions {
            validate_action(action)?;
        }
    }

    Ok(())
}

fn validate_action(action: &RecommendedAction) -> Result<()> {
    match action {
        RecommendedAction::RunCommand { parent, command } => {
            ensure_non_empty("parent", parent)?;
            ensure_non_empty("command", command)?;
        }
        RecommendedAction::SendPrompt { parent, prompt } => {
            ensure_non_empty("parent", parent)?;
            ensure_non_empty("prompt", prompt)?;
        }
        RecommendedAction::CreateShellTab { .. } => {}
        RecommendedAction::CreateShellPanel {
            parent, direction, ..
        } => {
            ensure_non_empty("parent", parent)?;
            normalize_direction(direction.as_deref())?;
        }
        RecommendedAction::DelegateTab { agent, prompt, .. } => {
            ensure_non_empty("agent", agent)?;
            ensure_non_empty("prompt", prompt)?;
        }
    }

    Ok(())
}

fn lookup_delegate_agent<'a>(
    delegate_agents: &'a [DelegateAgentRuntime],
    id: &str,
) -> Result<&'a DelegateAgentRuntime> {
    delegate_agents
        .iter()
        .find(|agent| agent.id == id)
        .ok_or_else(|| anyhow!("unsupported delegate agent '{}'", id))
}

fn build_delegate_commandline(runtime: &DelegateAgentRuntime, prompt: &str) -> Result<String> {
    ensure_non_empty("prompt", prompt)?;
    let normalized_prompt = prompt.replace("\r\n", "\n");
    let mut args = Vec::with_capacity(5);
    args.push(runtime.command.as_str());
    if let Some(model) = runtime.model.as_deref() {
        args.push("--model");
        args.push(model);
    }
    args.push("-i");
    args.push(normalized_prompt.as_str());
    Ok(join_windows_commandline(&args))
}

async fn send_shell_command_to_new_pane(
    shell_mgr: &ShellManager,
    pane_id: &str,
    command: &str,
) -> Result<()> {
    ensure_non_empty("commandline", command)?;
    sleep(Duration::from_millis(700)).await;
    shell_mgr
        .wt_send_input(pane_id, &format!("{command}\r"))
        .await
        .with_context(|| format!("failed to send command to pane {}", pane_id))?;
    Ok(())
}

fn join_windows_commandline(args: &[&str]) -> String {
    args.iter()
        .map(|arg| quote_windows_commandline_arg(arg))
        .collect::<Vec<_>>()
        .join(" ")
}

// Quote arguments using the standard Windows CommandLineToArgvW escaping rules.
fn quote_windows_commandline_arg(arg: &str) -> String {
    if arg.is_empty() {
        return "\"\"".to_string();
    }

    let needs_quotes = arg.chars().any(|ch| ch.is_whitespace() || ch == '"');
    if !needs_quotes {
        return arg.to_string();
    }

    let mut quoted = String::with_capacity(arg.len() + 2);
    quoted.push('"');
    let mut backslashes = 0usize;
    for ch in arg.chars() {
        match ch {
            '\\' => backslashes += 1,
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes * 2 + 1));
                quoted.push('"');
                backslashes = 0;
            }
            _ => {
                if backslashes > 0 {
                    quoted.push_str(&"\\".repeat(backslashes));
                    backslashes = 0;
                }
                quoted.push(ch);
            }
        }
    }

    if backslashes > 0 {
        quoted.push_str(&"\\".repeat(backslashes * 2));
    }
    quoted.push('"');
    quoted
}

fn non_empty_text(value: Option<&str>) -> Option<&str> {
    value.and_then(|text| {
        if text.trim().is_empty() {
            None
        } else {
            Some(text)
        }
    })
}

fn normalize_direction(direction: Option<&str>) -> Result<Option<&str>> {
    match direction {
        None => Ok(None),
        Some("right" | "left" | "up" | "down" | "automatic") => Ok(direction),
        Some(other) => bail!("unsupported panel direction '{}'", other),
    }
}

fn ensure_non_empty(field: &str, value: &str) -> Result<()> {
    if value.trim().is_empty() {
        bail!("field '{}' must not be empty", field);
    }
    Ok(())
}

fn value_to_string(value: Option<&serde_json::Value>) -> Option<String> {
    match value {
        Some(serde_json::Value::String(s)) => Some(s.clone()),
        Some(serde_json::Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

fn extract_json_code_block(text: &str) -> Option<&str> {
    let start = text.find("```json").or_else(|| text.find("```JSON"))?;
    let after_marker = &text[start + 7..];
    let trimmed = after_marker.strip_prefix('\r').unwrap_or(after_marker);
    let trimmed = trimmed.strip_prefix('\n').unwrap_or(trimmed);
    let end = trimmed.find("```")?;
    Some(trimmed[..end].trim())
}

fn extract_first_json_object(text: &str) -> Option<&str> {
    let start = text.find('{')?;
    let end = text.rfind('}')?;
    if end <= start {
        return None;
    }
    Some(text[start..=end].trim())
}

#[cfg(test)]
mod tests {
    use super::{
        build_delegate_commandline, default_delegate_agent_runtimes, parse_recommendation_set,
        RecommendedAction,
    };

    #[test]
    fn default_delegate_runtime_uses_cli_default_model() {
        let runtime = default_delegate_agent_runtimes()
            .into_iter()
            .find(|runtime| runtime.id == "copilot")
            .expect("copilot runtime should exist");

        assert_eq!(runtime.model, None);
    }

    #[test]
    fn delegate_commandline_omits_model_when_not_configured() {
        let runtime = default_delegate_agent_runtimes()
            .into_iter()
            .find(|runtime| runtime.id == "copilot")
            .expect("copilot runtime should exist");

        let commandline =
            build_delegate_commandline(&runtime, "Investigate the failing test").unwrap();

        assert!(!commandline.contains("--model"));
        assert!(commandline.contains("-i \"Investigate the failing test\""));
    }

    #[test]
    fn parse_recommendations_accepts_tab_actions_without_parent() {
        let text = r#"```json
{
  "recommended_choice": 1,
  "choices": [
    {
      "choice": 1,
      "title": "Open a shell tab",
      "actions": [
        {
          "type": "create_shell_tab",
          "cwd": "C:\\repo",
          "title": "Repo shell"
        }
      ]
    },
    {
      "choice": 2,
      "title": "Delegate in a new tab",
      "actions": [
        {
          "type": "delegate_tab",
          "agent": "copilot",
          "cwd": "C:\\repo",
          "prompt": "Inspect the repo",
          "title": "Copilot delegate"
        }
      ]
    },
    {
      "choice": 3,
      "title": "Run locally",
      "actions": [
        {
          "type": "run_command",
          "parent": "1",
          "command": "pwd"
        }
      ]
    }
  ]
}
```"#;

        let parsed = parse_recommendation_set(text).expect("recommendation set should parse");

        assert!(matches!(
            parsed.choices[0].actions[0],
            RecommendedAction::CreateShellTab { .. }
        ));
        assert!(matches!(
            parsed.choices[1].actions[0],
            RecommendedAction::DelegateTab { .. }
        ));
    }
}
