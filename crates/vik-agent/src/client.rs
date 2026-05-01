use std::future::Future;
use std::path::Path;

use serde_json::json;
use vik_core::{AgentEvent, LiveSession};
use vik_workflow::CodexConfig;

use crate::error::AgentError;
use crate::event::agent_event;
use crate::process::JsonlRpcProcess;

const CONTINUATION_PROMPT: &str = "Continue working on this Linear issue. Check current issue state and proceed only if it is still active.";

#[derive(Debug, Clone)]
pub struct CodexAppServerClient {
    command: String,
    config: CodexConfig,
}

#[derive(Debug, Clone)]
pub struct CodexIssueContext {
    pub issue_id: String,
    pub title: String,
}

impl CodexAppServerClient {
    pub fn new(config: CodexConfig) -> Self {
        Self {
            command: build_codex_command(&config),
            config,
        }
    }

    pub async fn run_turns<F, Fut>(
        &self,
        workspace_path: &Path,
        issue: CodexIssueContext,
        first_prompt: String,
        max_turns: u32,
        mut should_continue: F,
        mut on_event: impl FnMut(AgentEvent) + Send,
    ) -> Result<(), AgentError>
    where
        F: FnMut() -> Fut + Send,
        Fut: Future<Output = Result<bool, AgentError>> + Send,
    {
        if !workspace_path.is_absolute() {
            return Err(AgentError::InvalidWorkspaceCwd);
        }
        let mut process = JsonlRpcProcess::spawn(&self.command, workspace_path).await?;
        process.initialize().await?;
        let thread_id = process
            .thread_start(workspace_path, &issue.title, &self.config)
            .await?;
        let mut turn_count = 0_u32;
        loop {
            turn_count += 1;
            let prompt = if turn_count == 1 {
                first_prompt.clone()
            } else {
                CONTINUATION_PROMPT.to_string()
            };
            let turn_id = process
                .turn_start(&thread_id, workspace_path, prompt, &self.config)
                .await?;
            let mut live = LiveSession::new(thread_id.clone(), turn_id.clone());
            live.turn_count = turn_count;
            live.codex_app_server_pid = process.child.id().map(|pid| pid.to_string());
            on_event(agent_event(
                issue.issue_id.clone(),
                "session_started",
                Some(live.clone()),
                None,
                None,
                json!({ "thread_id": thread_id, "turn_id": turn_id }),
            ));
            process
                .wait_for_turn(
                    &thread_id,
                    &turn_id,
                    &mut live,
                    &issue.issue_id,
                    &mut on_event,
                )
                .await?;
            if turn_count >= max_turns || !should_continue().await? {
                break;
            }
        }
        let _ = process
            .request("thread/unsubscribe", json!({ "threadId": thread_id }))
            .await;
        let _ = process.child.kill().await;
        Ok(())
    }
}

pub(crate) fn build_codex_command(config: &CodexConfig) -> String {
    let dynamic_args = dynamic_config_args(config);
    if dynamic_args.is_empty() {
        return config.command.clone();
    }

    let command = config.command.trim_end();
    if let Some(prefix) = command.strip_suffix(" app-server") {
        format!(
            "{} {} app-server",
            prefix.trim_end(),
            dynamic_args.join(" ")
        )
    } else {
        format!("{command} {}", dynamic_args.join(" "))
    }
}

fn dynamic_config_args(config: &CodexConfig) -> Vec<String> {
    [
        ("model", config.model.as_deref()),
        (
            "model_reasoning_effort",
            config.model_reasoning_effort.as_deref(),
        ),
    ]
    .into_iter()
    .filter_map(|(key, value)| value.and_then(|value| codex_config_arg(key, value)))
    .collect()
}

fn codex_config_arg(key: &str, value: &str) -> Option<String> {
    let value = value.trim();
    if value.is_empty() {
        return None;
    }
    Some(format!(
        "--config {}",
        shell_quote(&format!("{key}={}", toml_string(value)))
    ))
}

fn toml_string(value: &str) -> String {
    serde_json::to_string(value).expect("string serialization should not fail")
}

fn shell_quote(value: &str) -> String {
    let mut quoted = String::from("'");
    for ch in value.chars() {
        if ch == '\'' {
            quoted.push_str("'\\''");
        } else {
            quoted.push(ch);
        }
    }
    quoted.push('\'');
    quoted
}
