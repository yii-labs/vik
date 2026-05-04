use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use vik_core::AgentEvent;
use vik_workflow::{CodingAgentKind, ServiceConfig};

pub(crate) mod claude_code;
pub(crate) mod codex;

use self::claude_code::ClaudeCodeClient;
use self::codex::{CodexAdapter, CodexAppServerClient};
use crate::error::AgentError;
use crate::tools::DynamicTools;

pub(crate) type ContinueFuture = Pin<Box<dyn Future<Output = Result<bool, AgentError>> + Send>>;
pub(crate) type ContinueCheck = Box<dyn FnMut() -> ContinueFuture + Send>;
pub(crate) type EventSink = Box<dyn FnMut(AgentEvent) + Send>;

pub(crate) struct CodingAgentRun {
    pub(crate) workspace_path: PathBuf,
    pub(crate) issue_id: String,
    pub(crate) issue_title: String,
    pub(crate) prompt: String,
    pub(crate) max_turns: u32,
    pub(crate) should_continue: ContinueCheck,
    pub(crate) on_event: EventSink,
}

#[async_trait]
pub(crate) trait CodingAgentAdapter: Send + Sync {
    fn kind(&self) -> CodingAgentKind;

    async fn run(&self, request: CodingAgentRun) -> Result<(), AgentError>;
}

pub(crate) fn adapter_for(
    kind: CodingAgentKind,
    config: &ServiceConfig,
    tools: DynamicTools,
) -> Box<dyn CodingAgentAdapter> {
    match kind {
        CodingAgentKind::Codex => Box::new(CodexAdapter {
            client: CodexAppServerClient::new(config.codex.clone()).with_dynamic_tools(tools),
        }),
        CodingAgentKind::ClaudeCode => Box::new(ClaudeCodeClient::new(
            config.claude_code.clone(),
            config.codex.stall_timeout_ms,
        )),
    }
}
