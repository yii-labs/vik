use std::path::Path;

use async_trait::async_trait;
use vik_core::{AgentEvent, AgentSession};
use vik_workflow::CodexConfig;

use crate::codex::process::{
    JsonlRpcProcess, ProcessCommand, SessionLogContext, TurnStartResponse,
};
use crate::codex::tools::DynamicTools;
use crate::error::AgentError;

pub(crate) type EventSink<'a> = &'a mut (dyn FnMut(AgentEvent) + Send);

#[async_trait]
pub(crate) trait CodexTransportFactory: Send + Sync + 'static {
    async fn spawn(
        &self,
        command: &ProcessCommand,
        cwd: &Path,
        config: &CodexConfig,
        tools: DynamicTools,
    ) -> Result<Box<dyn CodexTransport>, AgentError>;
}

#[async_trait]
pub(crate) trait CodexTransport: Send {
    fn process_id(&self) -> Option<String>;

    async fn initialize(&mut self) -> Result<(), AgentError>;

    async fn thread_start(
        &mut self,
        cwd: &Path,
        title: &str,
        config: &CodexConfig,
    ) -> Result<String, AgentError>;

    async fn turn_start(
        &mut self,
        thread_id: &str,
        cwd: &Path,
        prompt: String,
        config: &CodexConfig,
    ) -> Result<TurnStartResponse, AgentError>;

    fn set_session_log_context(&mut self, context: SessionLogContext);

    async fn wait_for_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        live: &mut AgentSession,
        issue_id: &str,
        on_event: EventSink<'_>,
    ) -> Result<(), AgentError>;

    async fn unsubscribe(&mut self, thread_id: &str);

    async fn shutdown(&mut self);
}

pub(crate) struct ProcessTransportFactory;

#[async_trait]
impl CodexTransportFactory for ProcessTransportFactory {
    async fn spawn(
        &self,
        command: &ProcessCommand,
        cwd: &Path,
        config: &CodexConfig,
        tools: DynamicTools,
    ) -> Result<Box<dyn CodexTransport>, AgentError> {
        let mut process = JsonlRpcProcess::spawn(command, cwd, tools).await?;
        process.configure_timeouts(config);
        Ok(Box::new(process))
    }
}

#[async_trait]
impl CodexTransport for JsonlRpcProcess {
    fn process_id(&self) -> Option<String> {
        self.child.id().map(|pid| pid.to_string())
    }

    async fn initialize(&mut self) -> Result<(), AgentError> {
        JsonlRpcProcess::initialize(self).await
    }

    async fn thread_start(
        &mut self,
        cwd: &Path,
        title: &str,
        config: &CodexConfig,
    ) -> Result<String, AgentError> {
        JsonlRpcProcess::thread_start(self, cwd, title, config).await
    }

    async fn turn_start(
        &mut self,
        thread_id: &str,
        cwd: &Path,
        prompt: String,
        config: &CodexConfig,
    ) -> Result<TurnStartResponse, AgentError> {
        JsonlRpcProcess::turn_start(self, thread_id, cwd, prompt, config).await
    }

    fn set_session_log_context(&mut self, context: SessionLogContext) {
        JsonlRpcProcess::set_session_log_context(self, context);
    }

    async fn wait_for_turn(
        &mut self,
        thread_id: &str,
        turn_id: &str,
        live: &mut AgentSession,
        issue_id: &str,
        on_event: EventSink<'_>,
    ) -> Result<(), AgentError> {
        JsonlRpcProcess::wait_for_turn(self, thread_id, turn_id, live, issue_id, on_event).await
    }

    async fn unsubscribe(&mut self, thread_id: &str) {
        let _ = self
            .request(
                "thread/unsubscribe",
                serde_json::json!({ "threadId": thread_id }),
            )
            .await;
    }

    async fn shutdown(&mut self) {
        let _ = self.child.kill().await;
    }
}
