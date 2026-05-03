use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time;
use vik_core::{AgentEvent, HostPlatform, LiveSession, PosixShell, ShellInvocation, TokenUsage};
use vik_workflow::{ClaudeCodeConfig, CodingAgentKind};

use crate::adapter::{CodingAgentAdapter, CodingAgentRun};
use crate::error::AgentError;
use crate::event::{agent_event, truncate};
use crate::process::ProcessCommand;

#[derive(Debug, Clone)]
pub(crate) struct ClaudeCodeClient {
    config: ClaudeCodeConfig,
}

impl ClaudeCodeClient {
    pub(crate) fn new(config: ClaudeCodeConfig) -> Self {
        Self { config }
    }
}

#[async_trait]
impl CodingAgentAdapter for ClaudeCodeClient {
    fn kind(&self) -> CodingAgentKind {
        CodingAgentKind::ClaudeCode
    }

    async fn run(&self, request: CodingAgentRun) -> Result<(), AgentError> {
        run_claude_code(&self.config, request).await
    }
}

async fn run_claude_code(
    config: &ClaudeCodeConfig,
    request: CodingAgentRun,
) -> Result<(), AgentError> {
    if !request.workspace_path.is_absolute() {
        return Err(AgentError::InvalidWorkspaceCwd);
    }

    let command_display = claude_code_spawn_command(config, request.max_turns);
    let command = claude_code_spawn_process_command(config, request.max_turns);
    let CodingAgentRun {
        workspace_path,
        issue_id,
        issue_title,
        prompt,
        on_event,
        should_continue: _,
        max_turns: _,
    } = request;
    let mut on_event = on_event;

    emit_lifecycle_event(
        &mut on_event,
        &issue_id,
        None,
        "claude_code_process_starting",
        json!({ "command": command_display }),
    );
    let mut process = Command::new(command.program());
    process
        .args(command.args())
        .current_dir(&workspace_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = process.spawn().map_err(|err| AgentError::ProcessSpawn {
        program: command.program().to_string(),
        reason: err.to_string(),
    })?;
    let pid = child.id().map(|pid| pid.to_string());
    let mut live = LiveSession::new(
        "claude-code",
        pid.as_deref()
            .map(|pid| format!("process-{pid}"))
            .unwrap_or_else(|| format!("process-{}", Utc::now().timestamp_millis())),
    );
    live.codex_app_server_pid = pid.clone();
    live.turn_count = 1;

    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(stream="stderr", message=%truncate(&line), "claude_code");
            }
        });
    }

    emit_lifecycle_event(
        &mut on_event,
        &issue_id,
        Some(live.clone()),
        "claude_code_process_started",
        json!({ "pid": pid }),
    );
    emit_lifecycle_event(
        &mut on_event,
        &issue_id,
        Some(live.clone()),
        "claude_code_turn_started",
        json!({ "title": issue_title, "cwd": workspace_path.display().to_string() }),
    );

    let mut stdin = child.stdin.take().ok_or(AgentError::PortExit)?;
    stdin
        .write_all(prompt.as_bytes())
        .await
        .map_err(|err| AgentError::ResponseError(err.to_string()))?;
    stdin
        .shutdown()
        .await
        .map_err(|err| AgentError::ResponseError(err.to_string()))?;

    let stdout = child.stdout.take().ok_or(AgentError::PortExit)?;
    let mut lines = BufReader::new(stdout).lines();
    let deadline = time::Instant::now() + Duration::from_millis(config.turn_timeout_ms);
    loop {
        let Some(remaining) = remaining_time(deadline) else {
            let _ = child.kill().await;
            return Err(AgentError::TurnTimeout);
        };
        match time::timeout(remaining, lines.next_line()).await {
            Ok(Ok(Some(line))) => {
                if line.trim().is_empty() {
                    continue;
                }
                let raw = serde_json::from_str::<Value>(&line)
                    .unwrap_or_else(|_| json!({ "text": line }));
                if let Some(usage) = claude_code_usage(&raw) {
                    live.codex_input_tokens = usage.input_tokens;
                    live.codex_output_tokens = usage.output_tokens;
                    live.codex_total_tokens = usage.total_tokens;
                }
                let event = claude_code_event_name(&raw);
                on_event(agent_event(
                    issue_id.clone(),
                    event,
                    Some(live.clone()),
                    claude_code_usage(&raw),
                    None,
                    raw,
                ));
            }
            Ok(Ok(None)) => break,
            Ok(Err(err)) => return Err(AgentError::ResponseError(err.to_string())),
            Err(_) => {
                let _ = child.kill().await;
                return Err(AgentError::TurnTimeout);
            }
        }
    }

    let Some(remaining) = remaining_time(deadline) else {
        let _ = child.kill().await;
        return Err(AgentError::TurnTimeout);
    };
    let status = time::timeout(remaining, child.wait())
        .await
        .map_err(|_| AgentError::TurnTimeout)?
        .map_err(|err| AgentError::ResponseError(err.to_string()))?;
    if !status.success() {
        return Err(AgentError::ProcessExit {
            program: command.program().to_string(),
            status: status.to_string(),
        });
    }

    emit_lifecycle_event(
        &mut on_event,
        &issue_id,
        Some(live),
        "claude_code_turn_completed",
        json!({ "status": "completed" }),
    );
    Ok(())
}

pub(crate) fn claude_code_spawn_command(config: &ClaudeCodeConfig, max_turns: u32) -> String {
    let mut command = config.command.trim().to_string();
    if let Some(model) = config.model.as_deref().map(str::trim)
        && !model.is_empty()
    {
        append_raw_arg(&mut command, "--model");
        append_shell_arg(&mut command, model);
    }
    if let Some(mode) = config.permission_mode.as_deref().map(str::trim)
        && !mode.is_empty()
    {
        append_raw_arg(&mut command, "--permission-mode");
        append_shell_arg(&mut command, mode);
    }
    if max_turns > 0 {
        append_raw_arg(&mut command, "--max-turns");
        append_shell_arg(&mut command, &max_turns.to_string());
    }
    command
}

pub(crate) fn claude_code_spawn_process_command(
    config: &ClaudeCodeConfig,
    max_turns: u32,
) -> ProcessCommand {
    claude_code_spawn_process_command_for_platform(config, max_turns, HostPlatform::current())
}

pub(crate) fn claude_code_spawn_process_command_for_platform(
    config: &ClaudeCodeConfig,
    max_turns: u32,
    platform: HostPlatform,
) -> ProcessCommand {
    let command = claude_code_spawn_command(config, max_turns);
    let shell = ShellInvocation::for_platform(&command, platform, PosixShell::Bash);
    ProcessCommand::new(
        shell.program(),
        shell
            .args()
            .iter()
            .copied()
            .chain(std::iter::once(shell.command())),
    )
}

fn append_shell_arg(command: &mut String, value: &str) {
    append_raw_arg(command, &shell_single_quote(value));
}

fn append_raw_arg(command: &mut String, value: &str) {
    if !command.is_empty() {
        command.push(' ');
    }
    command.push_str(value);
}

fn shell_single_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}

fn remaining_time(deadline: time::Instant) -> Option<Duration> {
    let now = time::Instant::now();
    (now < deadline).then_some(deadline - now)
}

fn emit_lifecycle_event(
    on_event: &mut impl FnMut(AgentEvent),
    issue_id: &str,
    session: Option<LiveSession>,
    event: &'static str,
    raw: Value,
) {
    on_event(agent_event(
        issue_id.to_string(),
        event,
        session,
        None,
        None,
        raw,
    ));
}

fn claude_code_event_name(raw: &Value) -> String {
    raw.get("type")
        .and_then(Value::as_str)
        .map(|kind| format!("claude_code_{kind}"))
        .unwrap_or_else(|| "claude_code_output".to_string())
}

fn claude_code_usage(raw: &Value) -> Option<TokenUsage> {
    let input = first_u64(raw, &["input_tokens", "inputTokens", "input"]);
    let output = first_u64(raw, &["output_tokens", "outputTokens", "output"]);
    let total = first_u64(raw, &["total_tokens", "totalTokens", "total"]);
    (input.is_some() || output.is_some() || total.is_some()).then_some(TokenUsage {
        input_tokens: input.unwrap_or(0),
        output_tokens: output.unwrap_or(0),
        total_tokens: total.unwrap_or_else(|| input.unwrap_or(0) + output.unwrap_or(0)),
    })
}

fn first_u64(value: &Value, keys: &[&str]) -> Option<u64> {
    for key in keys {
        if let Some(value) = value.get(*key).and_then(Value::as_u64) {
            return Some(value);
        }
    }
    if let Some(obj) = value.as_object() {
        for child in obj.values() {
            if let Some(value) = first_u64(child, keys) {
                return Some(value);
            }
        }
    }
    None
}
