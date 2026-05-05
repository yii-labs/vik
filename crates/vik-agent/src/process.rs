use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time;
use vik_core::{AgentEvent, LiveSession};
use vik_workflow::CodexConfig;

use crate::SESSION_LOG_TARGET;
use crate::error::AgentError;
use crate::event::{agent_event, extract_rate_limits, extract_usage, summarize_message, truncate};
use crate::tools::DynamicTools;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProcessCommand {
    program: String,
    args: Vec<String>,
}

impl ProcessCommand {
    pub(crate) fn new(
        program: impl Into<String>,
        args: impl IntoIterator<Item = impl Into<String>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
        }
    }

    pub(crate) fn program(&self) -> &str {
        &self.program
    }

    pub(crate) fn args(&self) -> &[String] {
        &self.args
    }
}

pub(crate) struct JsonlRpcProcess {
    pub(crate) child: Child,
    stdin: ChildStdin,
    stdout: Lines<BufReader<ChildStdout>>,
    next_id: u64,
    read_timeout: Duration,
    turn_timeout: Duration,
    tools: DynamicTools,
    pub(crate) session_log_context: SessionLogContext,
    pending_methods: HashMap<u64, String>,
}

pub(crate) struct TurnStartResponse {
    pub(crate) turn_id: String,
}

#[derive(Debug, Clone)]
pub(crate) struct SessionLogContext {
    issue_id: String,
    issue_identifier: String,
    session_id: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
}

impl SessionLogContext {
    pub(crate) fn new(issue_id: String, issue_identifier: String) -> Self {
        Self {
            issue_id,
            issue_identifier,
            session_id: None,
            thread_id: None,
            turn_id: None,
        }
    }

    fn set_thread(&mut self, thread_id: String) {
        self.thread_id = Some(thread_id);
    }

    pub(crate) fn set_live_session(&mut self, session: &LiveSession) {
        self.session_id = Some(session.session_id.clone());
        self.thread_id = Some(session.thread_id.clone());
        self.turn_id = Some(session.turn_id.clone());
    }

    pub(crate) fn clear_live_session(&mut self) {
        self.session_id = None;
        self.turn_id = None;
    }
}

pub(crate) struct SessionLogFields {
    pub(crate) event: String,
    pub(crate) message_kind: &'static str,
    pub(crate) rpc_id: Option<String>,
    pub(crate) params: Value,
}

impl JsonlRpcProcess {
    pub(crate) async fn spawn(
        command: &ProcessCommand,
        cwd: &Path,
        tools: DynamicTools,
        session_log_context: SessionLogContext,
    ) -> Result<Self, AgentError> {
        let mut process = Command::new(command.program());
        process
            .args(command.args())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let mut child = process.spawn().map_err(|err| AgentError::ProcessSpawn {
            program: command.program().to_string(),
            reason: err.to_string(),
        })?;
        let stdin = child.stdin.take().ok_or(AgentError::PortExit)?;
        let stdout = child.stdout.take().ok_or(AgentError::PortExit)?;
        if let Some(stderr) = child.stderr.take() {
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    tracing::debug!(stream="stderr", message=%truncate(&line), "codex_app_server");
                }
            });
        }
        Ok(Self {
            child,
            stdin,
            stdout: BufReader::new(stdout).lines(),
            next_id: 1,
            read_timeout: Duration::from_millis(5_000),
            turn_timeout: Duration::from_millis(3_600_000),
            tools,
            session_log_context,
            pending_methods: HashMap::new(),
        })
    }

    pub(crate) async fn initialize(&mut self) -> Result<(), AgentError> {
        self.request(
            "initialize",
            json!({
                "clientInfo": {
                    "name": "vik",
                    "title": "Vik",
                    "version": env!("CARGO_PKG_VERSION")
                },
                "capabilities": {
                    "experimentalApi": true
                }
            }),
        )
        .await?;
        self.write_message(&json!({ "method": "initialized" }))
            .await?;
        Ok(())
    }

    pub(crate) fn configure_timeouts(&mut self, config: &CodexConfig) {
        self.read_timeout = Duration::from_millis(config.read_timeout_ms);
        self.turn_timeout = Duration::from_millis(config.turn_timeout_ms);
    }

    pub(crate) async fn thread_start(
        &mut self,
        cwd: &Path,
        title: &str,
        config: &CodexConfig,
    ) -> Result<String, AgentError> {
        self.configure_timeouts(config);
        let params = thread_start_params(cwd, title, config, &self.tools);
        let response = self.request("thread/start", params).await?;
        let thread_id = response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| AgentError::ResponseError("missing thread.id".to_string()))?;
        self.session_log_context.set_thread(thread_id.clone());
        Ok(thread_id)
    }

    pub(crate) async fn turn_start(
        &mut self,
        thread_id: &str,
        cwd: &Path,
        prompt: String,
        config: &CodexConfig,
    ) -> Result<TurnStartResponse, AgentError> {
        let params = turn_start_params(thread_id, cwd, prompt, config);
        let response = self.request_message("turn/start", params).await?;
        let turn_id = response
            .pointer("/result/turn/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| AgentError::ResponseError("missing turn.id".to_string()))?;
        Ok(TurnStartResponse { turn_id })
    }

    pub(crate) async fn request(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, AgentError> {
        let response = self.request_message(method, params).await?;
        Ok(response.get("result").cloned().unwrap_or(Value::Null))
    }

    async fn request_message(&mut self, method: &str, params: Value) -> Result<Value, AgentError> {
        let id = self.next_id;
        self.next_id += 1;
        let request = json!({ "id": id, "method": method, "params": params });
        self.write_message(&request).await?;
        self.pending_methods.insert(id, method.to_string());
        loop {
            let message = self.read_message(self.read_timeout).await?;
            if message.get("id").and_then(Value::as_u64) == Some(id) {
                self.pending_methods.remove(&id);
                if let Some(error) = message.get("error") {
                    return Err(AgentError::ResponseError(error.to_string()));
                }
                return Ok(message);
            }
            if message.get("id").is_some() && message.get("method").is_some() {
                self.respond_to_server_request(&message).await?;
            }
        }
    }

    pub(crate) async fn wait_for_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        live: &mut LiveSession,
        issue_id: &str,
        on_event: &mut impl FnMut(AgentEvent),
    ) -> Result<(), AgentError> {
        let deadline = time::Instant::now() + self.turn_timeout;
        loop {
            let now = time::Instant::now();
            if now >= deadline {
                return Err(AgentError::TurnTimeout);
            }
            let timeout = deadline - now;
            let message = self.read_message(timeout).await?;
            if message.get("id").is_some() && message.get("method").is_some() {
                self.respond_to_server_request(&message).await?;
                continue;
            }
            let method = message
                .get("method")
                .and_then(Value::as_str)
                .unwrap_or("other_message");
            live.last_codex_event = Some(method.to_string());
            live.last_codex_timestamp = Some(Utc::now());
            live.last_codex_message = summarize_message(&message);
            if let Some(usage) = extract_usage(method, &message) {
                live.codex_input_tokens = usage.input_tokens;
                live.codex_output_tokens = usage.output_tokens;
                live.codex_total_tokens = usage.total_tokens;
            }
            on_event(agent_event(
                issue_id.to_string(),
                method,
                Some(live.clone()),
                extract_usage(method, &message),
                extract_rate_limits(method, &message),
                message.clone(),
            ));
            if method == "turn/completed"
                && message.pointer("/params/threadId").and_then(Value::as_str) == Some(thread_id)
            {
                let completed_turn = message.pointer("/params/turn/id").and_then(Value::as_str);
                if completed_turn != Some(turn_id) {
                    continue;
                }
                let status = message
                    .pointer("/params/turn/status")
                    .and_then(Value::as_str)
                    .unwrap_or("failed");
                return match status {
                    "completed" => Ok(()),
                    "interrupted" => Err(AgentError::TurnCancelled),
                    "failed" => Err(AgentError::TurnFailed(
                        message
                            .pointer("/params/turn/error/message")
                            .and_then(Value::as_str)
                            .unwrap_or("turn failed")
                            .to_string(),
                    )),
                    other => Err(AgentError::TurnFailed(other.to_string())),
                };
            }
        }
    }

    async fn respond_to_server_request(&mut self, message: &Value) -> Result<(), AgentError> {
        let Some(id) = message.get("id").cloned() else {
            return Ok(());
        };
        let method = message
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default();
        if matches!(
            method,
            "item/tool/requestUserInput"
                | "mcpServer/elicitation/request"
                | "account/chatgptAuthTokens/refresh"
        ) {
            return self
                .write_message(&json!({
                    "id": id,
                    "error": {
                        "code": -32603,
                        "message": format!("{method} is not handled by Vik policy")
                    }
                }))
                .await;
        }
        let result = match method {
            "item/commandExecution/requestApproval" => json!({ "decision": "acceptForSession" }),
            "item/fileChange/requestApproval" => json!({ "decision": "acceptForSession" }),
            "item/permissions/requestApproval" => {
                permission_approval_result(message.get("params").unwrap_or(&Value::Null))
            }
            "item/tool/call" => {
                self.tools
                    .handle_call(message.get("params").unwrap_or(&Value::Null))
                    .await
            }
            _ => json!({}),
        };
        self.write_message(&json!({ "id": id, "result": result }))
            .await
    }

    async fn write_message(&mut self, message: &Value) -> Result<(), AgentError> {
        let mut line = serde_json::to_vec(message)
            .map_err(|err| AgentError::ResponseError(err.to_string()))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|err| AgentError::ResponseError(err.to_string()))?;
        log_session_message(&self.session_log_context, None, "sent", message);
        Ok(())
    }

    async fn read_message(&mut self, timeout: Duration) -> Result<Value, AgentError> {
        let line = time::timeout(timeout, self.stdout.next_line())
            .await
            .map_err(|_| AgentError::ResponseTimeout)?
            .map_err(|err| AgentError::ResponseError(err.to_string()))?
            .ok_or(AgentError::PortExit)?;
        let message: Value = serde_json::from_str(&line)
            .map_err(|err| AgentError::ResponseError(err.to_string()))?;
        let pending_method = message
            .get("id")
            .and_then(Value::as_u64)
            .and_then(|id| self.pending_methods.get(&id).map(String::as_str));
        log_session_message(
            &self.session_log_context,
            pending_method,
            "received",
            &message,
        );
        Ok(message)
    }
}

fn log_session_message(
    context: &SessionLogContext,
    pending_method: Option<&str>,
    direction: &'static str,
    message: &Value,
) {
    let fields = session_log_fields(message, pending_method);
    let rpc_id = fields.rpc_id.as_deref().unwrap_or_default();
    let session_id = context.session_id.as_deref().unwrap_or_default();
    let thread_id = context.thread_id.as_deref().unwrap_or_default();
    let turn_id = context.turn_id.as_deref().unwrap_or_default();
    let params_json = fields.params.to_string();
    tracing::info!(
        target: SESSION_LOG_TARGET,
        category = "session",
        agent = "codex",
        issue_id = context.issue_id.as_str(),
        issue_identifier = context.issue_identifier.as_str(),
        session_id,
        thread_id,
        turn_id,
        direction,
        message_kind = fields.message_kind,
        event = fields.event.as_str(),
        rpc_id,
        params_json = params_json.as_str(),
        "agent_session_message"
    );
}

pub(crate) fn session_log_fields(
    message: &Value,
    pending_method: Option<&str>,
) -> SessionLogFields {
    let method = message.get("method").and_then(Value::as_str);
    let event = method
        .or(pending_method)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "message".to_string());
    let message_kind = if method.is_some() {
        "method"
    } else if message.get("error").is_some() {
        "rpc_error"
    } else if message.get("result").is_some() {
        "rpc_response"
    } else {
        "message"
    };
    let rpc_id = message.get("id").and_then(|id| match id {
        Value::Number(number) => Some(number.to_string()),
        Value::String(value) => Some(value.clone()),
        _ => None,
    });
    let params = message
        .get("params")
        .or_else(|| message.get("result"))
        .or_else(|| message.get("error"))
        .cloned()
        .unwrap_or_else(|| message.clone());
    SessionLogFields {
        event,
        message_kind,
        rpc_id,
        params,
    }
}

pub(crate) fn thread_start_params(
    cwd: &Path,
    title: &str,
    config: &CodexConfig,
    tools: &DynamicTools,
) -> Value {
    let mut params = json!({
        "cwd": cwd,
        "ephemeral": true,
        "approvalPolicy": config.approval_policy,
        "approvalsReviewer": config.approvals_reviewer,
        "sandbox": config.thread_sandbox,
        "sessionStartSource": "startup",
        "serviceName": title
    });
    let definitions = tools.definitions();
    if !definitions.is_empty() {
        params["dynamicTools"] = Value::Array(definitions);
    }
    params
}

pub(crate) fn permission_approval_result(params: &Value) -> Value {
    json!({
        "scope": "session",
        "permissions": params.get("permissions").cloned().unwrap_or_else(|| json!({}))
    })
}

pub(crate) fn turn_start_params(
    thread_id: &str,
    cwd: &Path,
    prompt: String,
    config: &CodexConfig,
) -> Value {
    json!({
        "threadId": thread_id,
        "cwd": cwd,
        "approvalsReviewer": config.approvals_reviewer,
        "sandboxPolicy": normalize_turn_sandbox_policy(cwd, config.turn_sandbox_policy.clone()),
        "input": [
            { "type": "text", "text": prompt }
        ]
    })
}

fn normalize_turn_sandbox_policy(cwd: &Path, policy: Option<Value>) -> Option<Value> {
    let Some(Value::Object(mut map)) = policy else {
        return policy;
    };
    if map.get("type").and_then(Value::as_str) == Some("workspaceWrite") {
        let cwd = cwd.to_string_lossy().to_string();
        match map.get_mut("writableRoots") {
            Some(Value::Array(roots)) => {
                if !roots.iter().any(|root| root.as_str() == Some(&cwd)) {
                    roots.push(Value::String(cwd));
                }
            }
            None => {
                map.insert("writableRoots".to_string(), json!([cwd]));
            }
            Some(_) => {}
        }
        map.entry("networkAccess".to_string())
            .or_insert(json!(true));
        map.entry("excludeTmpdirEnvVar".to_string())
            .or_insert(json!(false));
        map.entry("excludeSlashTmp".to_string())
            .or_insert(json!(false));
    }
    Some(Value::Object(map))
}
