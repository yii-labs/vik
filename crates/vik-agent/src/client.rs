use std::future::Future;
use std::path::Path;

use serde_json::json;
use vik_core::{AgentEvent, HostPlatform, LiveSession, PosixShell, ShellInvocation};
use vik_workflow::CodexConfig;

use crate::error::AgentError;
use crate::event::agent_event;
use crate::process::JsonlRpcProcess;
use crate::tools::DynamicTools;

const CONTINUATION_PROMPT: &str = "Continue working on this Linear issue. Check current issue state and proceed only if it is still active.";

#[derive(Debug, Clone)]
pub struct CodexAppServerClient {
    config: CodexConfig,
    tools: DynamicTools,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CodexSpawnCommand {
    pub(crate) program: String,
    pub(crate) args: Vec<String>,
}

impl CodexSpawnCommand {
    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }
}

#[derive(Debug, Clone)]
pub struct CodexIssueContext {
    pub issue_id: String,
    pub title: String,
}

impl CodexAppServerClient {
    pub fn new(config: CodexConfig) -> Self {
        Self {
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
            "codex_process_starting",
            json!({}),
        );
        let command = codex_spawn_process_command(&self.config)?;
        let mut process =
            JsonlRpcProcess::spawn(&command, workspace_path, self.tools.clone()).await?;
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

pub(crate) fn codex_spawn_process_command(
    config: &CodexConfig,
) -> Result<CodexSpawnCommand, AgentError> {
    codex_spawn_process_command_for_platform(config, HostPlatform::current())
}

pub(crate) fn codex_spawn_process_command_for_platform(
    config: &CodexConfig,
    platform: HostPlatform,
) -> Result<CodexSpawnCommand, AgentError> {
    match platform {
        HostPlatform::Windows => codex_spawn_direct_command(config),
        HostPlatform::Posix => {
            let command = codex_spawn_command(config);
            let shell = ShellInvocation::for_platform(&command, platform, PosixShell::Bash);
            Ok(CodexSpawnCommand {
                program: shell.program().to_string(),
                args: shell
                    .args()
                    .iter()
                    .map(|arg| (*arg).to_string())
                    .chain(std::iter::once(command))
                    .collect(),
            })
        }
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

fn codex_spawn_direct_command(config: &CodexConfig) -> Result<CodexSpawnCommand, AgentError> {
    let raw_command = config.command.trim();
    let mut argv = split_windows_command_line(raw_command);
    if argv.is_empty() {
        return Err(AgentError::InvalidCodexCommand(
            "codex.command is empty".to_string(),
        ));
    }

    let program = argv.remove(0);
    let args = codex_model_config_process_args(config);
    if args.is_empty() {
        return Ok(CodexSpawnCommand {
            program,
            args: argv,
        });
    }

    if let Some(index) = argv.iter().position(|arg| arg == "app-server") {
        argv.splice(index..index, args);
    } else {
        argv.extend(args);
    }
    Ok(CodexSpawnCommand {
        program,
        args: argv,
    })
}

fn split_windows_command_line(command: &str) -> Vec<String> {
    let mut args = Vec::new();
    let mut current = String::new();
    let mut backslashes = 0_usize;
    let mut in_quotes = false;
    let mut saw_arg = false;

    for ch in command.chars() {
        match ch {
            '\\' => {
                backslashes += 1;
                saw_arg = true;
            }
            '"' => {
                current.extend(std::iter::repeat_n('\\', backslashes / 2));
                if backslashes.is_multiple_of(2) {
                    in_quotes = !in_quotes;
                    saw_arg = true;
                } else {
                    current.push('"');
                    saw_arg = true;
                }
                backslashes = 0;
            }
            ch if ch.is_whitespace() && !in_quotes => {
                current.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                if saw_arg {
                    args.push(std::mem::take(&mut current));
                    saw_arg = false;
                }
            }
            ch => {
                current.extend(std::iter::repeat_n('\\', backslashes));
                backslashes = 0;
                current.push(ch);
                saw_arg = true;
            }
        }
    }

    current.extend(std::iter::repeat_n('\\', backslashes));
    if saw_arg {
        args.push(current);
    }
    args
}

fn codex_model_config_shell_args(config: &CodexConfig) -> Vec<String> {
    codex_model_config_values(config)
        .into_iter()
        .map(|value| format!("--config {}", shell_single_quote(&value)))
        .collect()
}

fn codex_model_config_process_args(config: &CodexConfig) -> Vec<String> {
    let mut args = Vec::new();
    for value in codex_model_config_values(config) {
        args.push("--config".to_string());
        args.push(value);
    }
    args
}

fn codex_model_config_values(config: &CodexConfig) -> Vec<String> {
    let mut values = Vec::new();
    if let Some(model) = config.model.as_deref().map(str::trim)
        && !model.is_empty()
    {
        values.push(format!("model={}", toml_string(model)));
    }
    if let Some(effort) = config.model_reasoning_effort.as_deref().map(str::trim)
        && !effort.is_empty()
    {
        values.push(format!("model_reasoning_effort={effort}"));
    }
    values
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
