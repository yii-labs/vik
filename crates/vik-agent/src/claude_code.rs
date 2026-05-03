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

use crate::adapter::EventSink;
use crate::adapter::{CodingAgentAdapter, CodingAgentRun};
use crate::error::AgentError;
use crate::event::{agent_event, truncate};
use crate::process::ProcessCommand;

const CONTINUATION_PROMPT: &str = "Continue working on this Linear issue. Check current issue state and proceed only if it is still active.";
const HEARTBEAT_INTERVAL_SECS: u64 = 30;

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

    let CodingAgentRun {
        workspace_path,
        issue_id,
        issue_title,
        prompt: first_prompt,
        on_event,
        should_continue,
        max_turns,
    } = request;
    let mut on_event = on_event;
    let mut should_continue = should_continue;
    let original_prompt = first_prompt.clone();
    let mut prompt = first_prompt;

    for turn_count in 1..=max_turns {
        run_claude_code_turn(
            config,
            ClaudeCodeTurn {
                workspace_path: &workspace_path,
                issue_id: &issue_id,
                issue_title: &issue_title,
                prompt,
                turn_count,
            },
            &mut on_event,
        )
        .await?;
        if turn_count >= max_turns || !should_continue().await? {
            break;
        }
        prompt = claude_code_continuation_prompt(&original_prompt);
    }
    Ok(())
}

struct ClaudeCodeTurn<'a> {
    workspace_path: &'a std::path::Path,
    issue_id: &'a str,
    issue_title: &'a str,
    prompt: String,
    turn_count: u32,
}

async fn run_claude_code_turn(
    config: &ClaudeCodeConfig,
    turn: ClaudeCodeTurn<'_>,
    on_event: &mut EventSink,
) -> Result<(), AgentError> {
    let command_display = claude_code_spawn_command(config, 1);
    let command = claude_code_spawn_process_command(config, 1);

    emit_lifecycle_event(
        on_event,
        turn.issue_id,
        None,
        "claude_code_process_starting",
        json!({ "command": command_display, "turn_count": turn.turn_count }),
    );
    let mut process = Command::new(command.program());
    process
        .args(command.args())
        .current_dir(turn.workspace_path)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    process.kill_on_drop(true);
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
    live.turn_count = turn.turn_count;

    if let Some(stderr) = child.stderr.take() {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr).lines();
            while let Ok(Some(line)) = lines.next_line().await {
                tracing::debug!(stream="stderr", message=%truncate(&line), "claude_code");
            }
        });
    }

    emit_lifecycle_event(
        on_event,
        turn.issue_id,
        Some(live.clone()),
        "claude_code_process_started",
        json!({ "pid": pid }),
    );
    emit_lifecycle_event(
        on_event,
        turn.issue_id,
        Some(live.clone()),
        "claude_code_turn_started",
        json!({
            "title": turn.issue_title,
            "cwd": turn.workspace_path.display().to_string(),
            "turn_count": turn.turn_count,
        }),
    );

    let mut stdin = child.stdin.take().ok_or(AgentError::PortExit)?;
    stdin
        .write_all(turn.prompt.as_bytes())
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
        let wait = remaining.min(heartbeat_interval());
        match time::timeout(wait, lines.next_line()).await {
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
                    turn.issue_id.to_string(),
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
                if remaining <= heartbeat_interval() {
                    let _ = child.kill().await;
                    return Err(AgentError::TurnTimeout);
                }
                emit_lifecycle_event(
                    on_event,
                    turn.issue_id,
                    Some(live.clone()),
                    "claude_code_heartbeat",
                    json!({ "turn_count": turn.turn_count }),
                );
            }
        }
    }

    let status = loop {
        if let Some(status) = child
            .try_wait()
            .map_err(|err| AgentError::ResponseError(err.to_string()))?
        {
            break status;
        }
        let Some(remaining) = remaining_time(deadline) else {
            let _ = child.kill().await;
            return Err(AgentError::TurnTimeout);
        };
        let wait = remaining.min(heartbeat_interval());
        time::sleep(wait).await;
        if remaining <= heartbeat_interval() {
            let _ = child.kill().await;
            return Err(AgentError::TurnTimeout);
        }
        emit_lifecycle_event(
            on_event,
            turn.issue_id,
            Some(live.clone()),
            "claude_code_heartbeat",
            json!({ "turn_count": turn.turn_count }),
        );
    };
    if !status.success() {
        return Err(AgentError::ProcessExit {
            program: command.program().to_string(),
            status: status.to_string(),
        });
    }

    emit_lifecycle_event(
        on_event,
        turn.issue_id,
        Some(live),
        "claude_code_turn_completed",
        json!({ "status": "completed", "turn_count": turn.turn_count }),
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
    match platform {
        HostPlatform::Posix => {
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
        HostPlatform::Windows => claude_code_spawn_direct_command(config, max_turns),
    }
}

fn claude_code_spawn_direct_command(config: &ClaudeCodeConfig, max_turns: u32) -> ProcessCommand {
    let mut argv = split_windows_command_line(&config.command);
    if argv.is_empty() {
        return ProcessCommand::new(config.command.trim(), std::iter::empty::<String>());
    }
    argv.extend(claude_code_runtime_args(config, max_turns));
    let program = argv.remove(0);
    ProcessCommand::new(program, argv)
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

fn claude_code_runtime_args(config: &ClaudeCodeConfig, max_turns: u32) -> Vec<String> {
    let mut args = Vec::new();
    if let Some(model) = config.model.as_deref().map(str::trim)
        && !model.is_empty()
    {
        args.push("--model".to_string());
        args.push(model.to_string());
    }
    if let Some(mode) = config.permission_mode.as_deref().map(str::trim)
        && !mode.is_empty()
    {
        args.push("--permission-mode".to_string());
        args.push(mode.to_string());
    }
    if max_turns > 0 {
        args.push("--max-turns".to_string());
        args.push(max_turns.to_string());
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

fn remaining_time(deadline: time::Instant) -> Option<Duration> {
    let now = time::Instant::now();
    (now < deadline).then_some(deadline - now)
}

fn heartbeat_interval() -> Duration {
    Duration::from_secs(HEARTBEAT_INTERVAL_SECS)
}

fn claude_code_continuation_prompt(original_prompt: &str) -> String {
    format!("{CONTINUATION_PROMPT}\n\nOriginal issue prompt:\n\n{original_prompt}")
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
