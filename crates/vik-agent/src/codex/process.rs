use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use chrono::Utc;
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, Lines};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::time;
use vik_core::{AgentEvent, AgentSession, session_id};
use vik_workflow::CodexConfig;

use crate::SESSION_LOG_TARGET;
use crate::codex::events::{
    agent_event, extract_rate_limits, extract_usage, summarize_message, truncate,
};
use crate::codex::tools::DynamicTools;
use crate::error::AgentError;

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
    pending_methods: HashMap<String, String>,
    read_timeout: Duration,
    turn_timeout: Duration,
    tools: DynamicTools,
    session_log_context: SessionLogContext,
}

pub(crate) struct TurnStartResponse {
    pub(crate) turn_id: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct SessionLogContext {
    issue_id: String,
    issue_identifier: String,
    session_id: Option<String>,
    thread_id: Option<String>,
    turn_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct SessionLogFields {
    pub(crate) event: String,
    pub(crate) params: Value,
    pub(crate) rpc_id: Option<String>,
}

impl JsonlRpcProcess {
    pub(crate) async fn spawn(
        command: &ProcessCommand,
        cwd: &Path,
        tools: DynamicTools,
    ) -> Result<Self, AgentError> {
        let tools = tools.with_workspace_root(cwd.to_path_buf());
        let mut process = Command::new(command.program());
        process
            .args(command.args())
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        process.kill_on_drop(true);
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
            pending_methods: HashMap::new(),
            read_timeout: Duration::from_millis(5_000),
            turn_timeout: Duration::from_millis(3_600_000),
            tools,
            session_log_context: SessionLogContext::default(),
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
        response
            .pointer("/thread/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| AgentError::ResponseError("missing thread.id".to_string()))
    }

    pub(crate) async fn turn_start(
        &mut self,
        thread_id: &str,
        cwd: &Path,
        prompt: String,
        config: &CodexConfig,
    ) -> Result<TurnStartResponse, AgentError> {
        let params = turn_start_params(thread_id, cwd, prompt, config);
        let (response, unmatched_messages) = self
            .request_message_collecting_unmatched("turn/start", params)
            .await?;
        let turn_id = response
            .pointer("/result/turn/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| AgentError::ResponseError("missing turn.id".to_string()))?;
        let session_context = SessionLogContext::for_session(
            self.session_log_context.issue_id.clone(),
            self.session_log_context.issue_identifier.clone(),
            thread_id.to_string(),
            turn_id.clone(),
        );
        log_turn_start_received_messages(&session_context, &unmatched_messages, &response);
        Ok(TurnStartResponse { turn_id })
    }

    pub(crate) fn set_session_log_context(&mut self, context: SessionLogContext) {
        self.session_log_context = context;
    }

    pub(crate) fn log_session_message(&self, direction: &'static str, message: &Value) {
        let rpc_id = message.get("id").and_then(rpc_id_string);
        let pending_method = rpc_id
            .as_deref()
            .and_then(|id| self.pending_methods.get(id).map(String::as_str));
        log_session_message(
            &self.session_log_context,
            pending_method,
            direction,
            message,
        );
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
        let (response, _) = self.request_message_inner(method, params, false).await?;
        Ok(response)
    }

    async fn request_message_collecting_unmatched(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<(Value, Vec<PendingReceivedMessage>), AgentError> {
        self.request_message_inner(method, params, true).await
    }

    async fn request_message_inner(
        &mut self,
        method: &str,
        params: Value,
        collect_unmatched: bool,
    ) -> Result<(Value, Vec<PendingReceivedMessage>), AgentError> {
        let id = self.next_id;
        self.next_id += 1;
        let request_rpc_id = id.to_string();
        let request = json!({ "id": id, "method": method, "params": params });
        self.pending_methods
            .insert(request_rpc_id.clone(), method.to_string());
        self.write_message(&request).await?;
        let mut unmatched_messages = Vec::new();
        loop {
            let message = self.read_message_raw(self.read_timeout).await?;
            let matches_request = message.get("id").and_then(Value::as_u64) == Some(id);
            if !collect_unmatched {
                self.log_session_message("received", &message);
            }
            if matches_request {
                if let Some(error) = message.get("error") {
                    if collect_unmatched {
                        log_session_message(
                            &self.session_log_context,
                            Some(method),
                            "received",
                            &message,
                        );
                    }
                    self.pending_methods.remove(&request_rpc_id);
                    return Err(AgentError::ResponseError(error.to_string()));
                }
                self.pending_methods.remove(&request_rpc_id);
                return Ok((message, unmatched_messages));
            }
            if collect_unmatched {
                unmatched_messages.push(self.pending_received_message(&message));
            }
            if message.get("id").is_some() && message.get("method").is_some() {
                self.respond_to_server_request(&message).await?;
            }
        }
    }

    fn pending_received_message(&self, message: &Value) -> PendingReceivedMessage {
        PendingReceivedMessage {
            message: message.clone(),
            pending_method: self.pending_method_for(message).map(ToOwned::to_owned),
        }
    }

    fn pending_method_for<'a>(&'a self, message: &Value) -> Option<&'a str> {
        let rpc_id = message.get("id").and_then(rpc_id_string)?;
        self.pending_methods.get(&rpc_id).map(String::as_str)
    }

    pub(crate) async fn wait_for_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        live: &mut AgentSession,
        issue_id: &str,
        on_event: &mut (dyn FnMut(AgentEvent) + Send),
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
            live.last_event = Some(method.to_string());
            live.last_event_at = Some(Utc::now());
            live.last_message = summarize_message(&message);
            if let Some(usage) = extract_usage(method, &message) {
                live.input_tokens = usage.input_tokens;
                live.output_tokens = usage.output_tokens;
                live.total_tokens = usage.total_tokens;
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
            let rpc_id = rpc_id_string(&id);
            if let Some(rpc_id) = &rpc_id {
                self.pending_methods
                    .insert(rpc_id.clone(), method.to_string());
            }
            let result = self
                .write_message(&json!({
                    "id": id,
                    "error": {
                        "code": -32603,
                        "message": format!("{method} is not handled by Vik policy")
                    }
                }))
                .await;
            if let Some(rpc_id) = rpc_id {
                self.pending_methods.remove(&rpc_id);
            }
            return result;
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
        let rpc_id = rpc_id_string(&id);
        if let Some(rpc_id) = &rpc_id {
            self.pending_methods
                .insert(rpc_id.clone(), method.to_string());
        }
        let result = self
            .write_message(&json!({ "id": id, "result": result }))
            .await;
        if let Some(rpc_id) = rpc_id {
            self.pending_methods.remove(&rpc_id);
        }
        result
    }

    async fn write_message(&mut self, message: &Value) -> Result<(), AgentError> {
        let mut line = serde_json::to_vec(message)
            .map_err(|err| AgentError::ResponseError(err.to_string()))?;
        line.push(b'\n');
        self.stdin
            .write_all(&line)
            .await
            .map_err(|err| AgentError::ResponseError(err.to_string()))?;
        self.log_session_message("sent", message);
        Ok(())
    }

    async fn read_message(&mut self, timeout: Duration) -> Result<Value, AgentError> {
        let message = self.read_message_raw(timeout).await?;
        self.log_session_message("received", &message);
        Ok(message)
    }

    async fn read_message_raw(&mut self, timeout: Duration) -> Result<Value, AgentError> {
        let line = time::timeout(timeout, self.stdout.next_line())
            .await
            .map_err(|_| AgentError::ResponseTimeout)?
            .map_err(|err| AgentError::ResponseError(err.to_string()))?
            .ok_or(AgentError::PortExit)?;
        serde_json::from_str(&line).map_err(|err| AgentError::ResponseError(err.to_string()))
    }
}

#[derive(Debug, Clone, PartialEq)]
struct PendingReceivedMessage {
    message: Value,
    pending_method: Option<String>,
}

impl SessionLogContext {
    pub(crate) fn new(issue_id: impl Into<String>, issue_identifier: impl Into<String>) -> Self {
        Self {
            issue_id: issue_id.into(),
            issue_identifier: issue_identifier.into(),
            session_id: None,
            thread_id: None,
            turn_id: None,
        }
    }

    pub(crate) fn for_session(
        issue_id: impl Into<String>,
        issue_identifier: impl Into<String>,
        thread_id: impl Into<String>,
        turn_id: impl Into<String>,
    ) -> Self {
        let thread_id = thread_id.into();
        let turn_id = turn_id.into();
        Self {
            issue_id: issue_id.into(),
            issue_identifier: issue_identifier.into(),
            session_id: Some(session_id(&thread_id, &turn_id)),
            thread_id: Some(thread_id),
            turn_id: Some(turn_id),
        }
    }

    pub(crate) fn for_thread(
        issue_id: impl Into<String>,
        issue_identifier: impl Into<String>,
        thread_id: impl Into<String>,
    ) -> Self {
        Self {
            issue_id: issue_id.into(),
            issue_identifier: issue_identifier.into(),
            session_id: None,
            thread_id: Some(thread_id.into()),
            turn_id: None,
        }
    }

    pub(crate) fn identity_for<'a>(&'a self, message: &'a Value) -> SessionLogIdentity {
        let thread_id = message_thread_id(message).or(self.thread_id.as_deref());
        let turn_id = message_turn_id(message).or(self.turn_id.as_deref());
        let session_id = thread_id
            .zip(turn_id)
            .map(|(thread_id, turn_id)| session_id(thread_id, turn_id))
            .or_else(|| self.session_id.as_deref().map(ToOwned::to_owned));
        SessionLogIdentity {
            issue_id: self.issue_id.clone(),
            issue_identifier: self.issue_identifier.clone(),
            session_id: session_id.unwrap_or_default(),
            thread_id: thread_id.unwrap_or_default().to_string(),
            turn_id: turn_id.unwrap_or_default().to_string(),
        }
    }
}

pub(crate) struct SessionLogIdentity {
    pub(crate) issue_id: String,
    pub(crate) issue_identifier: String,
    pub(crate) session_id: String,
    pub(crate) thread_id: String,
    pub(crate) turn_id: String,
}

fn log_session_message(
    context: &SessionLogContext,
    pending_method: Option<&str>,
    direction: &'static str,
    message: &Value,
) {
    let fields = session_log_fields(message, pending_method);
    let identity = context.identity_for(message);
    let rpc_id = fields.rpc_id.as_deref().unwrap_or_default();
    let params_json = fields.params.to_string();
    tracing::info!(
        target: SESSION_LOG_TARGET,
        category = "session",
        agent = "codex",
        direction = direction,
        event = fields.event.as_str(),
        params_json = params_json.as_str(),
        issue_id = identity.issue_id.as_str(),
        issue_identifier = identity.issue_identifier.as_str(),
        session_id = identity.session_id.as_str(),
        thread_id = identity.thread_id.as_str(),
        turn_id = identity.turn_id.as_str(),
        rpc_id = rpc_id,
        "agent_session_message"
    );
}

fn log_turn_start_received_messages(
    context: &SessionLogContext,
    unmatched_messages: &[PendingReceivedMessage],
    response: &Value,
) {
    for message in unmatched_messages {
        log_session_message(
            context,
            message.pending_method.as_deref(),
            "received",
            &message.message,
        );
    }
    log_session_message(context, Some("turn/start"), "received", response);
}

pub(crate) fn session_log_fields(
    message: &Value,
    pending_method: Option<&str>,
) -> SessionLogFields {
    let event = message
        .get("method")
        .and_then(Value::as_str)
        .or(pending_method)
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| {
            if message.get("error").is_some() {
                "rpc_error".to_string()
            } else if message.get("result").is_some() {
                "rpc_response".to_string()
            } else {
                "message".to_string()
            }
        });
    let params = message
        .get("params")
        .or_else(|| message.get("result"))
        .or_else(|| message.get("error"))
        .cloned()
        .unwrap_or_else(|| message.clone());

    SessionLogFields {
        event,
        params,
        rpc_id: message.get("id").and_then(rpc_id_string),
    }
}

fn rpc_id_string(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(ToOwned::to_owned)
        .or_else(|| value.as_u64().map(|id| id.to_string()))
}

fn message_thread_id(message: &Value) -> Option<&str> {
    message
        .pointer("/params/threadId")
        .or_else(|| message.pointer("/params/thread/id"))
        .or_else(|| message.pointer("/result/threadId"))
        .or_else(|| message.pointer("/result/thread/id"))
        .and_then(Value::as_str)
}

fn message_turn_id(message: &Value) -> Option<&str> {
    message
        .pointer("/params/turn/id")
        .or_else(|| message.pointer("/params/turnId"))
        .or_else(|| message.pointer("/result/turn/id"))
        .or_else(|| message.pointer("/result/turnId"))
        .and_then(Value::as_str)
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tracing::field::{Field, Visit};
    use tracing::span::{Attributes, Id, Record};
    use tracing::{Dispatch, Event, Metadata, Subscriber};

    #[test]
    fn turn_start_receive_logs_flush_buffered_messages_before_response() {
        let records = Arc::new(Mutex::new(Vec::new()));
        let dispatch = Dispatch::new(RecordingSubscriber {
            records: Arc::clone(&records),
        });
        let context = SessionLogContext::for_session("issue-1", "VIK-1", "thread-1", "turn-2");
        let buffered = vec![
            PendingReceivedMessage {
                message: json!({
                    "method": "turn/started",
                    "params": {
                        "threadId": "thread-1",
                        "turn": { "id": "turn-2" }
                    }
                }),
                pending_method: None,
            },
            PendingReceivedMessage {
                message: json!({ "id": 9, "result": { "ok": true } }),
                pending_method: Some("item/tool/call".to_string()),
            },
        ];
        let response = json!({ "id": 4, "result": { "turn": { "id": "turn-2" } } });

        tracing::dispatcher::with_default(&dispatch, || {
            log_turn_start_received_messages(&context, &buffered, &response);
        });

        assert_eq!(
            records.lock().unwrap().as_slice(),
            [
                RecordedSessionEvent {
                    event: "turn/started".to_string(),
                    turn_id: "turn-2".to_string(),
                    rpc_id: String::new(),
                },
                RecordedSessionEvent {
                    event: "item/tool/call".to_string(),
                    turn_id: "turn-2".to_string(),
                    rpc_id: "9".to_string(),
                },
                RecordedSessionEvent {
                    event: "turn/start".to_string(),
                    turn_id: "turn-2".to_string(),
                    rpc_id: "4".to_string(),
                },
            ]
        );
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct RecordedSessionEvent {
        event: String,
        turn_id: String,
        rpc_id: String,
    }

    struct RecordingSubscriber {
        records: Arc<Mutex<Vec<RecordedSessionEvent>>>,
    }

    impl Subscriber for RecordingSubscriber {
        fn enabled(&self, _metadata: &Metadata<'_>) -> bool {
            true
        }

        fn new_span(&self, _span: &Attributes<'_>) -> Id {
            Id::from_u64(1)
        }

        fn record(&self, _span: &Id, _values: &Record<'_>) {}

        fn record_follows_from(&self, _span: &Id, _follows: &Id) {}

        fn event(&self, event: &Event<'_>) {
            let mut visitor = RecordedEventVisitor::default();
            event.record(&mut visitor);
            self.records.lock().unwrap().push(RecordedSessionEvent {
                event: visitor.event.unwrap_or_default(),
                turn_id: visitor.turn_id.unwrap_or_default(),
                rpc_id: visitor.rpc_id.unwrap_or_default(),
            });
        }

        fn enter(&self, _span: &Id) {}

        fn exit(&self, _span: &Id) {}
    }

    #[derive(Default)]
    struct RecordedEventVisitor {
        event: Option<String>,
        turn_id: Option<String>,
        rpc_id: Option<String>,
    }

    impl Visit for RecordedEventVisitor {
        fn record_str(&mut self, field: &Field, value: &str) {
            match field.name() {
                "event" => self.event = Some(value.to_string()),
                "turn_id" => self.turn_id = Some(value.to_string()),
                "rpc_id" => self.rpc_id = Some(value.to_string()),
                _ => {}
            }
        }

        fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
            self.record_str(field, &format!("{value:?}"));
        }
    }
}
