use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use async_trait::async_trait;
use chrono::Utc;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::time;
use vik_core::{AgentEvent, HostPlatform, LiveSession, PosixShell, ShellInvocation, TokenUsage};
use vik_workflow::{ClaudeCodeConfig, CodingAgentKind};

use super::{CodingAgentAdapter, CodingAgentRun, EventSink};
use crate::error::AgentError;
use crate::event::{agent_event, truncate};
use crate::process::ProcessCommand;
use crate::tools::{DynamicTools, LinearGraphqlToolEnv};

const CONTINUATION_PROMPT: &str = "Continue working on this Linear issue. Check current issue state and proceed only if it is still active.";
const DEFAULT_HEARTBEAT_INTERVAL_MS: u64 = 30_000;
const VIK_LINEAR_GRAPHQL_MCP_TOOL: &str = "mcp__vik__linear_graphql";
const VIK_LINEAR_GRAPHQL_ENDPOINT_ENV: &str = "VIK_LINEAR_GRAPHQL_ENDPOINT";
const VIK_LINEAR_GRAPHQL_API_KEY_ENV: &str = "VIK_LINEAR_GRAPHQL_API_KEY";

#[derive(Debug, Clone)]
pub(crate) struct ClaudeCodeClient {
    config: ClaudeCodeConfig,
    heartbeat_interval: Duration,
    tools: DynamicTools,
}

impl ClaudeCodeClient {
    pub(crate) fn new(config: ClaudeCodeConfig, stall_timeout_ms: i64) -> Self {
        Self {
            config,
            heartbeat_interval: heartbeat_interval_for_stall_timeout(stall_timeout_ms),
            tools: DynamicTools::default(),
        }
    }

    pub(crate) fn with_dynamic_tools(mut self, tools: DynamicTools) -> Self {
        self.tools = tools;
        self
    }

    async fn run_request(&self, request: CodingAgentRun) -> Result<(), AgentError> {
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
        let mut usage = ClaudeUsageAccumulator::default();
        let mcp_runtime = ClaudeMcpRuntime::prepare(&self.tools).await?;

        for turn_count in 1..=max_turns {
            self.run_turn(
                ClaudeCodeTurn {
                    workspace_path: &workspace_path,
                    issue_id: &issue_id,
                    issue_title: &issue_title,
                    prompt,
                    turn_count,
                },
                &mut on_event,
                &mut usage,
                mcp_runtime.as_ref(),
            )
            .await?;
            usage.finish_turn();
            if turn_count >= max_turns || !should_continue().await? {
                break;
            }
            prompt = claude_code_continuation_prompt(&original_prompt);
        }
        Ok(())
    }
}

#[async_trait]
impl CodingAgentAdapter for ClaudeCodeClient {
    fn kind(&self) -> CodingAgentKind {
        CodingAgentKind::ClaudeCode
    }

    async fn run(&self, request: CodingAgentRun) -> Result<(), AgentError> {
        self.run_request(request).await
    }
}

struct ClaudeCodeTurn<'a> {
    workspace_path: &'a Path,
    issue_id: &'a str,
    issue_title: &'a str,
    prompt: String,
    turn_count: u32,
}

impl ClaudeCodeClient {
    async fn run_turn(
        &self,
        turn: ClaudeCodeTurn<'_>,
        on_event: &mut EventSink,
        usage: &mut ClaudeUsageAccumulator,
        mcp_runtime: Option<&ClaudeMcpRuntime>,
    ) -> Result<(), AgentError> {
        let mcp_config_path = mcp_runtime.map(|runtime| runtime.config_path.as_path());
        let command_display = claude_code_spawn_command_with_mcp(&self.config, 1, mcp_config_path);
        let command = claude_code_spawn_process_command_with_mcp(&self.config, 1, mcp_config_path);

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
        if let Some(runtime) = mcp_runtime {
            runtime.apply_env(&mut process);
        }
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

        let deadline = time::Instant::now() + Duration::from_millis(self.config.turn_timeout_ms);
        let stdin = child.stdin.take().ok_or(AgentError::PortExit)?;
        if let Err(err) = write_prompt_with_deadline(
            stdin,
            &turn.prompt,
            deadline,
            on_event,
            PromptWriteContext {
                issue_id: turn.issue_id,
                turn_count: turn.turn_count,
                live: &live,
                heartbeat_interval: self.heartbeat_interval(),
            },
        )
        .await
        {
            let _ = child.kill().await;
            return Err(err);
        }

        let stdout = child.stdout.take().ok_or(AgentError::PortExit)?;
        let mut lines = BufReader::new(stdout).lines();
        loop {
            let Some(remaining) = remaining_time(deadline) else {
                let _ = child.kill().await;
                return Err(AgentError::TurnTimeout);
            };
            let heartbeat_interval = self.heartbeat_interval();
            let wait = remaining.min(heartbeat_interval);
            match time::timeout(wait, lines.next_line()).await {
                Ok(Ok(Some(line))) => {
                    if line.trim().is_empty() {
                        continue;
                    }
                    let raw = serde_json::from_str::<Value>(&line)
                        .unwrap_or_else(|_| json!({ "text": line }));
                    let cumulative_usage =
                        claude_code_usage(&raw).map(|turn_usage| usage.update(turn_usage));
                    if let Some(cumulative_usage) = cumulative_usage {
                        live.codex_input_tokens = cumulative_usage.input_tokens;
                        live.codex_output_tokens = cumulative_usage.output_tokens;
                        live.codex_total_tokens = cumulative_usage.total_tokens;
                    }
                    let event = claude_code_event_name(&raw);
                    on_event(agent_event(
                        turn.issue_id.to_string(),
                        event,
                        Some(live.clone()),
                        cumulative_usage,
                        None,
                        raw,
                    ));
                }
                Ok(Ok(None)) => break,
                Ok(Err(err)) => return Err(AgentError::ResponseError(err.to_string())),
                Err(_) => {
                    if remaining <= heartbeat_interval {
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
            let wait = remaining.min(self.heartbeat_interval());
            time::sleep(wait).await;
            if let Some(status) = child
                .try_wait()
                .map_err(|err| AgentError::ResponseError(err.to_string()))?
            {
                break status;
            }
            if remaining_time(deadline).is_none() {
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

    fn heartbeat_interval(&self) -> Duration {
        self.heartbeat_interval
    }
}

struct ClaudeMcpRuntime {
    config_path: PathBuf,
    linear_graphql: LinearGraphqlToolEnv,
}

impl ClaudeMcpRuntime {
    async fn prepare(tools: &DynamicTools) -> Result<Option<Self>, AgentError> {
        let Some(linear_graphql) = tools.linear_graphql_env() else {
            return Ok(None);
        };
        let current_exe =
            std::env::current_exe().map_err(|err| AgentError::ResponseError(err.to_string()))?;
        let body = claude_mcp_config_body(&current_exe);
        let config_path = write_unique_claude_mcp_config(&body).await?;
        Ok(Some(Self {
            config_path,
            linear_graphql,
        }))
    }

    fn apply_env(&self, process: &mut Command) {
        process.env(
            VIK_LINEAR_GRAPHQL_ENDPOINT_ENV,
            &self.linear_graphql.endpoint,
        );
        process.env(VIK_LINEAR_GRAPHQL_API_KEY_ENV, &self.linear_graphql.api_key);
    }
}

impl Drop for ClaudeMcpRuntime {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.config_path);
    }
}

fn claude_mcp_config_body(current_exe: &Path) -> Value {
    json!({
        "mcpServers": {
            "vik": {
                "type": "stdio",
                "command": current_exe.display().to_string(),
                "args": ["mcp", "linear-graphql"],
                "env": {
                    "VIK_LINEAR_GRAPHQL_ENDPOINT": format!("${{{VIK_LINEAR_GRAPHQL_ENDPOINT_ENV}}}"),
                    "VIK_LINEAR_GRAPHQL_API_KEY": format!("${{{VIK_LINEAR_GRAPHQL_API_KEY_ENV}}}"),
                }
            }
        }
    })
}

async fn write_unique_claude_mcp_config(body: &Value) -> Result<PathBuf, AgentError> {
    let bytes =
        serde_json::to_vec(body).map_err(|err| AgentError::ResponseError(err.to_string()))?;
    for attempt in 0..100_u32 {
        let timestamp = Utc::now()
            .timestamp_nanos_opt()
            .map(|nanos| nanos.to_string())
            .unwrap_or_else(|| Utc::now().timestamp_millis().to_string());
        let path = std::env::temp_dir().join(format!(
            "vik-claude-mcp-{}-{timestamp}-{attempt}.json",
            std::process::id()
        ));
        let mut file = match tokio::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .await
        {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(err) => return Err(AgentError::ResponseError(err.to_string())),
        };
        if let Err(err) = file.write_all(&bytes).await {
            let _ = tokio::fs::remove_file(&path).await;
            return Err(AgentError::ResponseError(err.to_string()));
        }
        return Ok(path);
    }
    Err(AgentError::ResponseError(
        "failed to allocate unique Claude MCP config path".to_string(),
    ))
}

#[derive(Debug, Default)]
pub(crate) struct ClaudeUsageAccumulator {
    completed: TokenUsage,
    current_turn: TokenUsage,
}

impl ClaudeUsageAccumulator {
    pub(crate) fn update(&mut self, turn_usage: TokenUsage) -> TokenUsage {
        self.current_turn.input_tokens =
            self.current_turn.input_tokens.max(turn_usage.input_tokens);
        self.current_turn.output_tokens = self
            .current_turn
            .output_tokens
            .max(turn_usage.output_tokens);
        self.current_turn.total_tokens =
            self.current_turn.total_tokens.max(turn_usage.total_tokens);
        TokenUsage {
            input_tokens: self.completed.input_tokens + self.current_turn.input_tokens,
            output_tokens: self.completed.output_tokens + self.current_turn.output_tokens,
            total_tokens: self.completed.total_tokens + self.current_turn.total_tokens,
        }
    }

    pub(crate) fn finish_turn(&mut self) {
        self.completed.input_tokens += self.current_turn.input_tokens;
        self.completed.output_tokens += self.current_turn.output_tokens;
        self.completed.total_tokens += self.current_turn.total_tokens;
        self.current_turn = TokenUsage::default();
    }
}

pub(crate) async fn write_prompt_with_deadline<W>(
    mut stdin: W,
    prompt: &str,
    deadline: time::Instant,
    on_event: &mut EventSink,
    context: PromptWriteContext<'_>,
) -> Result<(), AgentError>
where
    W: AsyncWrite + Unpin,
{
    let write_prompt = async {
        stdin
            .write_all(prompt.as_bytes())
            .await
            .map_err(|err| AgentError::ResponseError(err.to_string()))?;
        stdin
            .shutdown()
            .await
            .map_err(|err| AgentError::ResponseError(err.to_string()))
    };
    tokio::pin!(write_prompt);
    loop {
        let Some(remaining) = remaining_time(deadline) else {
            return Err(AgentError::TurnTimeout);
        };
        let wait = remaining.min(context.heartbeat_interval);
        match time::timeout(wait, &mut write_prompt).await {
            Ok(result) => return result,
            Err(_) => {
                if remaining <= context.heartbeat_interval {
                    return Err(AgentError::TurnTimeout);
                }
                emit_lifecycle_event(
                    on_event,
                    context.issue_id,
                    Some(context.live.clone()),
                    "claude_code_heartbeat",
                    json!({ "turn_count": context.turn_count, "phase": "stdin" }),
                );
            }
        }
    }
}

pub(crate) struct PromptWriteContext<'a> {
    issue_id: &'a str,
    turn_count: u32,
    live: &'a LiveSession,
    heartbeat_interval: Duration,
}

#[cfg(test)]
pub(crate) fn claude_code_spawn_command(config: &ClaudeCodeConfig, max_turns: u32) -> String {
    claude_code_spawn_command_with_mcp(config, max_turns, None)
}

fn claude_code_spawn_command_with_mcp(
    config: &ClaudeCodeConfig,
    max_turns: u32,
    mcp_config_path: Option<&Path>,
) -> String {
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
    if let Some(path) = mcp_config_path {
        append_raw_arg(&mut command, "--mcp-config");
        append_shell_arg(&mut command, &path.display().to_string());
        append_raw_arg(&mut command, "--allowedTools");
        append_shell_arg(&mut command, VIK_LINEAR_GRAPHQL_MCP_TOOL);
    }
    if max_turns > 0 {
        append_raw_arg(&mut command, "--max-turns");
        append_shell_arg(&mut command, &max_turns.to_string());
    }
    command
}

fn claude_code_spawn_process_command_with_mcp(
    config: &ClaudeCodeConfig,
    max_turns: u32,
    mcp_config_path: Option<&Path>,
) -> ProcessCommand {
    claude_code_spawn_process_command_for_platform_with_mcp(
        config,
        max_turns,
        HostPlatform::current(),
        mcp_config_path,
    )
}

#[cfg(test)]
pub(crate) fn claude_code_spawn_process_command_for_platform(
    config: &ClaudeCodeConfig,
    max_turns: u32,
    platform: HostPlatform,
) -> ProcessCommand {
    claude_code_spawn_process_command_for_platform_with_mcp(config, max_turns, platform, None)
}

fn claude_code_spawn_process_command_for_platform_with_mcp(
    config: &ClaudeCodeConfig,
    max_turns: u32,
    platform: HostPlatform,
    mcp_config_path: Option<&Path>,
) -> ProcessCommand {
    match platform {
        HostPlatform::Posix => {
            let command = claude_code_spawn_command_with_mcp(config, max_turns, mcp_config_path);
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
        HostPlatform::Windows => {
            claude_code_spawn_direct_command(config, max_turns, mcp_config_path)
        }
    }
}

fn claude_code_spawn_direct_command(
    config: &ClaudeCodeConfig,
    max_turns: u32,
    mcp_config_path: Option<&Path>,
) -> ProcessCommand {
    let mut argv = split_windows_command_line(&config.command);
    if argv.is_empty() {
        return ProcessCommand::new(config.command.trim(), std::iter::empty::<String>());
    }
    argv.extend(claude_code_runtime_args(config, max_turns, mcp_config_path));
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

fn claude_code_runtime_args(
    config: &ClaudeCodeConfig,
    max_turns: u32,
    mcp_config_path: Option<&Path>,
) -> Vec<String> {
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
    if let Some(path) = mcp_config_path {
        args.push("--mcp-config".to_string());
        args.push(path.display().to_string());
        args.push("--allowedTools".to_string());
        args.push(VIK_LINEAR_GRAPHQL_MCP_TOOL.to_string());
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

fn heartbeat_interval_for_stall_timeout(stall_timeout_ms: i64) -> Duration {
    if stall_timeout_ms <= 0 {
        return Duration::from_millis(DEFAULT_HEARTBEAT_INTERVAL_MS);
    }
    let safe_interval_ms = ((stall_timeout_ms as u64) / 2).max(1);
    Duration::from_millis(DEFAULT_HEARTBEAT_INTERVAL_MS.min(safe_interval_ms))
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

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::time::Duration;

    use serde_json::json;
    use tokio::time;
    use vik_core::{HostPlatform, LiveSession, TokenUsage};
    use vik_workflow::ClaudeCodeConfig;

    use super::{
        ClaudeUsageAccumulator, PromptWriteContext, claude_code_spawn_command,
        claude_code_spawn_command_with_mcp, claude_code_spawn_process_command_for_platform,
        claude_code_spawn_process_command_for_platform_with_mcp, write_prompt_with_deadline,
    };
    use crate::agent::EventSink;
    use crate::error::AgentError;

    #[cfg(unix)]
    #[tokio::test]
    async fn claude_code_accepts_success_after_stdout_eof_near_deadline() {
        let workspace = tempfile::TempDir::new().unwrap();
        let client = super::ClaudeCodeClient::new(
            ClaudeCodeConfig {
                command: "sh -c 'exec 1>&-; cat >/dev/null; sleep 0.01; exit 0'".to_string(),
                turn_timeout_ms: 1000,
                ..ClaudeCodeConfig::default()
            },
            300_000,
        );

        let result = client
            .run_request(crate::agent::CodingAgentRun {
                workspace_path: workspace.path().to_path_buf(),
                issue_id: "issue-id".to_string(),
                issue_title: "VIK-37: multiple agents".to_string(),
                prompt: "test prompt".to_string(),
                max_turns: 1,
                should_continue: Box::new(|| Box::pin(async { Ok(false) })),
                on_event: Box::new(|_| {}),
            })
            .await;

        assert!(result.is_ok(), "{result:?}");
    }

    #[test]
    fn claude_code_spawn_command_adds_runtime_options() {
        let config = ClaudeCodeConfig {
            command: "claude -p --output-format stream-json --input-format text".to_string(),
            model: Some("sonnet".to_string()),
            permission_mode: Some("acceptEdits".to_string()),
            ..ClaudeCodeConfig::default()
        };

        assert_eq!(
            claude_code_spawn_command(&config, 7),
            "claude -p --output-format stream-json --input-format text --model 'sonnet' --permission-mode 'acceptEdits' --max-turns '7'"
        );
    }

    #[test]
    fn claude_code_spawn_command_adds_vik_mcp_bridge() {
        let config = ClaudeCodeConfig {
            command: "claude -p --output-format stream-json --input-format text".to_string(),
            ..ClaudeCodeConfig::default()
        };

        assert_eq!(
            claude_code_spawn_command_with_mcp(&config, 7, Some(Path::new("/tmp/vik mcp.json"))),
            "claude -p --output-format stream-json --input-format text --mcp-config '/tmp/vik mcp.json' --allowedTools 'mcp__vik__linear_graphql' --max-turns '7'"
        );
    }

    #[test]
    fn claude_code_heartbeat_interval_stays_below_stall_timeout() {
        assert_eq!(
            super::heartbeat_interval_for_stall_timeout(300_000),
            Duration::from_millis(30_000)
        );
        assert_eq!(
            super::heartbeat_interval_for_stall_timeout(10_000),
            Duration::from_millis(5_000)
        );
        assert_eq!(
            super::heartbeat_interval_for_stall_timeout(0),
            Duration::from_millis(30_000)
        );
    }

    #[test]
    fn claude_code_spawn_process_command_uses_shell() {
        let config = ClaudeCodeConfig {
            command: "claude -p --output-format stream-json --input-format text".to_string(),
            ..ClaudeCodeConfig::default()
        };
        let command =
            claude_code_spawn_process_command_for_platform(&config, 3, HostPlatform::Posix);

        assert_eq!(command.program(), "bash");
        assert_eq!(
            command.args(),
            &[
                "-lc".to_string(),
                "claude -p --output-format stream-json --input-format text --max-turns '3'"
                    .to_string()
            ]
        );
    }

    #[test]
    fn claude_code_spawn_process_command_uses_direct_windows_argv() {
        let config = ClaudeCodeConfig {
            command: r#""C:\Program Files\Claude\claude.exe" -p --output-format stream-json"#
                .to_string(),
            model: Some("o'hara".to_string()),
            permission_mode: Some("acceptEdits".to_string()),
            ..ClaudeCodeConfig::default()
        };
        let command =
            claude_code_spawn_process_command_for_platform(&config, 1, HostPlatform::Windows);

        assert_eq!(command.program(), r#"C:\Program Files\Claude\claude.exe"#);
        assert_eq!(
            command.args(),
            &[
                "-p".to_string(),
                "--output-format".to_string(),
                "stream-json".to_string(),
                "--model".to_string(),
                "o'hara".to_string(),
                "--permission-mode".to_string(),
                "acceptEdits".to_string(),
                "--max-turns".to_string(),
                "1".to_string(),
            ]
        );
    }

    #[test]
    fn claude_code_spawn_process_command_adds_vik_mcp_bridge_on_windows() {
        let config = ClaudeCodeConfig {
            command: r#""C:\Program Files\Claude\claude.exe" -p"#.to_string(),
            ..ClaudeCodeConfig::default()
        };
        let command = claude_code_spawn_process_command_for_platform_with_mcp(
            &config,
            1,
            HostPlatform::Windows,
            Some(Path::new(r#"C:\Temp\vik-mcp.json"#)),
        );

        assert_eq!(command.program(), r#"C:\Program Files\Claude\claude.exe"#);
        assert_eq!(
            command.args(),
            &[
                "-p".to_string(),
                "--mcp-config".to_string(),
                r#"C:\Temp\vik-mcp.json"#.to_string(),
                "--allowedTools".to_string(),
                "mcp__vik__linear_graphql".to_string(),
                "--max-turns".to_string(),
                "1".to_string(),
            ]
        );
    }

    #[test]
    fn claude_mcp_config_points_to_vik_linear_graphql_server_without_secret() {
        let config = super::claude_mcp_config_body(Path::new("/bin/vik"));

        assert_eq!(
            config.pointer("/mcpServers/vik/command"),
            Some(&json!("/bin/vik"))
        );
        assert_eq!(
            config.pointer("/mcpServers/vik/args"),
            Some(&json!(["mcp", "linear-graphql"]))
        );
        assert_eq!(
            config.pointer("/mcpServers/vik/env/VIK_LINEAR_GRAPHQL_API_KEY"),
            Some(&json!("${VIK_LINEAR_GRAPHQL_API_KEY}"))
        );
    }

    #[tokio::test]
    async fn claude_mcp_config_paths_are_created_atomically() {
        let config = super::claude_mcp_config_body(Path::new("/bin/vik"));

        let first = super::write_unique_claude_mcp_config(&config)
            .await
            .unwrap();
        let second = super::write_unique_claude_mcp_config(&config)
            .await
            .unwrap();

        assert_ne!(first, second);
        assert!(first.exists());
        assert!(second.exists());
        let _ = tokio::fs::remove_file(first).await;
        let _ = tokio::fs::remove_file(second).await;
    }

    #[tokio::test]
    async fn claude_code_prompt_write_obeys_turn_deadline() {
        let (writer, _reader) = tokio::io::duplex(1);
        let deadline = time::Instant::now() + Duration::from_millis(10);
        let live = LiveSession::new("claude-code", "process-test");
        let mut on_event: EventSink = Box::new(|_| {});

        let result = write_prompt_with_deadline(
            writer,
            &"x".repeat(8 * 1024),
            deadline,
            &mut on_event,
            PromptWriteContext {
                issue_id: "VIK-37",
                turn_count: 1,
                live: &live,
                heartbeat_interval: Duration::from_millis(1),
            },
        )
        .await;

        assert!(matches!(result, Err(AgentError::TurnTimeout)));
    }

    #[test]
    fn claude_code_usage_accumulator_reports_run_totals_across_turns() {
        let mut usage = ClaudeUsageAccumulator::default();

        assert_eq!(
            usage.update(TokenUsage {
                input_tokens: 10,
                output_tokens: 4,
                total_tokens: 14,
            }),
            TokenUsage {
                input_tokens: 10,
                output_tokens: 4,
                total_tokens: 14,
            }
        );
        assert_eq!(
            usage.update(TokenUsage {
                input_tokens: 8,
                output_tokens: 6,
                total_tokens: 14,
            }),
            TokenUsage {
                input_tokens: 10,
                output_tokens: 6,
                total_tokens: 14,
            }
        );

        usage.finish_turn();

        assert_eq!(
            usage.update(TokenUsage {
                input_tokens: 3,
                output_tokens: 2,
                total_tokens: 5,
            }),
            TokenUsage {
                input_tokens: 13,
                output_tokens: 8,
                total_tokens: 19,
            }
        );
    }
}
