use std::future::Future;
use std::path::Path;

use serde_json::json;
use vik_core::{AgentEvent, LiveSession};
use vik_workflow::CodexConfig;

use crate::error::AgentError;
use crate::event::agent_event;
use crate::process::JsonlRpcProcess;
use crate::tools::DynamicTools;

const CONTINUATION_PROMPT: &str = "Continue working on this Linear issue. Check current issue state and proceed only if it is still active.";

#[derive(Debug, Clone)]
pub struct CodexAppServerClient {
    command: String,
    config: CodexConfig,
    tools: DynamicTools,
}

#[derive(Debug, Clone)]
pub struct CodexIssueContext {
    pub issue_id: String,
    pub session_file_id: String,
    pub title: String,
}

impl CodexAppServerClient {
    pub fn new(config: CodexConfig) -> Self {
        Self {
            command: codex_spawn_command(&config),
            config,
            tools: DynamicTools::default(),
        }
    }

    pub(crate) fn with_dynamic_tools(mut self, tools: DynamicTools) -> Self {
        self.tools = tools;
        self
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
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            &issue.session_file_id,
            "codex_process_starting",
            json!({}),
        );
        let mut process =
            JsonlRpcProcess::spawn(&self.command, workspace_path, self.tools.clone()).await?;
        process.configure_timeouts(&self.config);
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            &issue.session_file_id,
            "codex_process_started",
            json!({ "pid": process.child.id() }),
        );
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            &issue.session_file_id,
            "codex_initialize_starting",
            json!({}),
        );
        process.initialize().await?;
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            &issue.session_file_id,
            "codex_initialize_completed",
            json!({}),
        );
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            &issue.session_file_id,
            "codex_thread_starting",
            json!({ "cwd": workspace_path.display().to_string() }),
        );
        let thread_id = process
            .thread_start(workspace_path, &issue.title, &self.config)
            .await?;
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            &issue.session_file_id,
            "codex_thread_started",
            json!({ "thread_id": &thread_id }),
        );
        let mut turn_count = 0_u32;
        loop {
            turn_count += 1;
            let prompt = if turn_count == 1 {
                first_prompt.clone()
            } else {
                CONTINUATION_PROMPT.to_string()
            };
            emit_lifecycle_event(
                &mut on_event,
                &issue.issue_id,
                &issue.session_file_id,
                "codex_turn_starting",
                json!({ "thread_id": &thread_id, "turn_count": turn_count }),
            );
            let turn_id = process
                .turn_start(&thread_id, workspace_path, prompt, &self.config)
                .await?;
            let mut live = LiveSession::new(thread_id.clone(), turn_id.clone());
            live.turn_count = turn_count;
            live.codex_app_server_pid = process.child.id().map(|pid| pid.to_string());
            on_event(agent_event(
                issue.issue_id.clone(),
                issue.session_file_id.clone(),
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
                    &issue.session_file_id,
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

pub(crate) fn codex_spawn_command(config: &CodexConfig) -> String {
    let args = codex_model_config_args(config);
    if args.is_empty() {
        return config.command.clone();
    }

    let joined_args = args.join(" ");
    if let Some((prefix, app_server_command)) = config.split_command_at_app_server() {
        let prefix = prefix.trim_end();
        let app_server_command = app_server_command.trim_start();
        if prefix.is_empty() {
            format!("{joined_args} {app_server_command}")
        } else {
            format!("{prefix} {joined_args} {app_server_command}")
        }
    } else {
        let command = config.command.trim();
        format!("{command} {joined_args}")
    }
}

fn codex_model_config_args(config: &CodexConfig) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = config.model.as_deref().map(str::trim)
        && !model.is_empty()
    {
        args.push(format!(
            "--config {}",
            shell_single_quote(&format!("model={}", toml_string(model)))
        ));
    }
    if let Some(effort) = config.model_reasoning_effort.as_deref().map(str::trim)
        && !effort.is_empty()
    {
        args.push(format!(
            "--config {}",
            shell_single_quote(&format!("model_reasoning_effort={effort}"))
        ));
    }
    args
}

fn toml_string(value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    quoted.push('"');
    for ch in value.chars() {
        match ch {
            '\\' => quoted.push_str("\\\\"),
            '"' => quoted.push_str("\\\""),
            '\n' => quoted.push_str("\\n"),
            '\r' => quoted.push_str("\\r"),
            '\t' => quoted.push_str("\\t"),
            _ => quoted.push(ch),
        }
    }
    quoted.push('"');
    quoted
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn emit_lifecycle_event(
    on_event: &mut impl FnMut(AgentEvent),
    issue_id: &str,
    session_file_id: &str,
    event: &'static str,
    raw: serde_json::Value,
) {
    on_event(agent_event(
        issue_id.to_string(),
        session_file_id.to_string(),
        event,
        None,
        None,
        None,
        raw,
    ));
}
