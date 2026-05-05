use std::future::Future;
use std::path::Path;

use serde_json::json;
use vik_core::{AgentEvent, HostPlatform, LiveSession, PosixShell, ShellInvocation};
use vik_workflow::CodexConfig;

use crate::error::AgentError;
use crate::event::agent_event;
use crate::process::{JsonlRpcProcess, ProcessCommand};
use crate::session_log::{SessionLog, session_log_dir, session_log_path};
use crate::tools::DynamicTools;

const CONTINUATION_PROMPT: &str = "Continue working on this tracker issue. Check current issue state and proceed only if it is still active.";

#[derive(Debug, Clone)]
pub struct CodexAppServerClient {
    command: ProcessCommand,
    config: CodexConfig,
    tools: DynamicTools,
}

#[derive(Debug, Clone)]
pub struct CodexIssueContext {
    pub issue_id: String,
    pub title: String,
}

impl CodexAppServerClient {
    pub fn new(config: CodexConfig) -> Self {
        Self {
            command: codex_spawn_process_command(&config),
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
        let session_log_dir = session_log_dir(session_workspace_root(workspace_path));
        let issue_identifier = session_issue_identifier(&issue).to_string();
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            "codex_process_starting",
            json!({}),
        );
        let mut process =
            JsonlRpcProcess::spawn(&self.command, workspace_path, self.tools.clone()).await?;
        process.configure_timeouts(&self.config);
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            "codex_process_started",
            json!({ "pid": process.child.id() }),
        );
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            "codex_initialize_starting",
            json!({}),
        );
        process.initialize().await?;
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            "codex_initialize_completed",
            json!({}),
        );
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            "codex_thread_starting",
            json!({ "cwd": workspace_path.display().to_string() }),
        );
        let thread_id = process
            .thread_start(workspace_path, &issue.title, &self.config)
            .await?;
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
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
                "codex_turn_starting",
                json!({ "thread_id": &thread_id, "turn_count": turn_count }),
            );
            let turn_start = process
                .turn_start(&thread_id, workspace_path, prompt, &self.config)
                .await?;
            let turn_id = turn_start.turn_id;
            let mut live = LiveSession::new(thread_id.clone(), turn_id.clone());
            live.turn_count = turn_count;
            live.codex_app_server_pid = process.child.id().map(|pid| pid.to_string());
            let session_log_path =
                session_log_path(&session_log_dir, &issue_identifier, &live.session_id);
            match SessionLog::open(session_log_path).await {
                Ok(mut session_log) => {
                    for message in &turn_start.pre_response_messages {
                        if message_belongs_to_turn(message, &turn_id) {
                            if let Err(err) = session_log.append_message(message).await {
                                tracing::warn!(
                                    path=%session_log.path().display(),
                                    error=%err,
                                    "codex_session_log_append outcome=failed"
                                );
                            }
                        } else {
                            process.append_current_session_message(message).await;
                        }
                    }
                    if let Err(err) = session_log.append_message(&turn_start.response).await {
                        tracing::warn!(
                            path=%session_log.path().display(),
                            error=%err,
                            "codex_session_log_append outcome=failed"
                        );
                    }
                    process.set_session_log(Some(session_log));
                }
                Err(err) => {
                    tracing::warn!(error=%err, "codex_session_log_open outcome=failed");
                    for message in &turn_start.pre_response_messages {
                        if !message_belongs_to_turn(message, &turn_id) {
                            process.append_current_session_message(message).await;
                        }
                    }
                    process.set_session_log(None);
                }
            }
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

pub(crate) fn codex_spawn_command(config: &CodexConfig) -> String {
    let args = codex_model_config_shell_args(config);
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

pub(crate) fn codex_spawn_process_command(config: &CodexConfig) -> ProcessCommand {
    codex_spawn_process_command_for_platform(config, HostPlatform::current())
}

pub(crate) fn codex_spawn_process_command_for_platform(
    config: &CodexConfig,
    platform: HostPlatform,
) -> ProcessCommand {
    match platform {
        HostPlatform::Posix => {
            let command = codex_spawn_command(config);
            let shell =
                ShellInvocation::for_platform(&command, HostPlatform::Posix, PosixShell::Bash);
            ProcessCommand::new(
                shell.program(),
                shell
                    .args()
                    .iter()
                    .copied()
                    .chain(std::iter::once(shell.command())),
            )
        }
        HostPlatform::Windows => codex_spawn_direct_command(config),
    }
}

fn codex_spawn_direct_command(config: &CodexConfig) -> ProcessCommand {
    let mut argv = split_windows_command_line(&config.command);
    if argv.is_empty() {
        return ProcessCommand::new(config.command.trim(), std::iter::empty::<String>());
    }

    let model_args = codex_model_config_argv(config);
    if !model_args.is_empty() {
        let insert_at = argv
            .iter()
            .position(|arg| arg == "app-server")
            .unwrap_or(argv.len());
        argv.splice(insert_at..insert_at, model_args);
    }

    let program = argv.remove(0);
    ProcessCommand::new(program, argv)
}

fn codex_model_config_shell_args(config: &CodexConfig) -> Vec<String> {
    codex_model_config_argv(config)
        .chunks_exact(2)
        .map(|pair| format!("{} {}", pair[0], shell_single_quote(&pair[1])))
        .collect()
}

fn codex_model_config_argv(config: &CodexConfig) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = config.model.as_deref().map(str::trim)
        && !model.is_empty()
    {
        args.push("--config".to_string());
        args.push(format!("model={}", toml_string(model)));
    }
    if let Some(effort) = config.model_reasoning_effort.as_deref().map(str::trim)
        && !effort.is_empty()
    {
        args.push("--config".to_string());
        args.push(format!("model_reasoning_effort={effort}"));
    }
    args
}

fn split_windows_command_line(input: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut in_quotes = false;
    let mut arg_started = false;
    let mut backslashes = 0;

    for ch in input.trim().chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                arg_started = true;
            }
            '"' => {
                arg_started = true;
                current.extend(std::iter::repeat_n('\\', backslashes / 2));
                if backslashes % 2 == 0 {
                    in_quotes = !in_quotes;
                } else {
                    current.push('"');
                }
                backslashes = 0;
            }
            ch if ch.is_whitespace() && !in_quotes => {
                current.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                if arg_started {
                    args.push(std::mem::take(&mut current));
                    arg_started = false;
                }
            }
            _ => {
                current.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                current.push(ch);
                arg_started = true;
            }
        }
    }

    current.extend(std::iter::repeat_n('\\', backslashes));
    if arg_started {
        args.push(current);
    }
    args
}

fn session_workspace_root(workspace_path: &Path) -> &Path {
    workspace_path.parent().unwrap_or(workspace_path)
}

fn session_issue_identifier(issue: &CodexIssueContext) -> &str {
    issue
        .title
        .split_once(':')
        .map(|(identifier, _)| identifier.trim())
        .filter(|identifier| !identifier.is_empty())
        .unwrap_or(&issue.issue_id)
}

pub(crate) fn message_belongs_to_turn(message: &serde_json::Value, turn_id: &str) -> bool {
    message_turn_id(message).is_none_or(|message_turn_id| message_turn_id == turn_id)
}

fn message_turn_id(message: &serde_json::Value) -> Option<&str> {
    message
        .pointer("/params/turn/id")
        .or_else(|| message.pointer("/params/turnId"))
        .or_else(|| message.pointer("/result/turn/id"))
        .and_then(serde_json::Value::as_str)
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
    event: &'static str,
    raw: serde_json::Value,
) {
    on_event(agent_event(
        issue_id.to_string(),
        event,
        None,
        None,
        None,
        raw,
    ));
}
