use std::future::Future;
use std::path::PathBuf;
use std::pin::Pin;

use async_trait::async_trait;
use vik_core::AgentEvent;
use vik_workflow::{CodingAgentKind, ServiceConfig};

use crate::claude_code::ClaudeCodeClient;
use crate::client::{CodexAppServerClient, CodexIssueContext};
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
        CodingAgentKind::ClaudeCode => Box::new(ClaudeCodeClient::new(config.claude_code.clone())),
    }
}

struct CodexAdapter {
    client: CodexAppServerClient,
}

#[async_trait]
impl CodingAgentAdapter for CodexAdapter {
    fn kind(&self) -> CodingAgentKind {
        CodingAgentKind::Codex
    }

    async fn run(&self, request: CodingAgentRun) -> Result<(), AgentError> {
        let CodingAgentRun {
            workspace_path,
            issue_id,
            issue_title,
            prompt,
            max_turns,
            should_continue,
            on_event,
        } = request;

        self.client
            .run_turns(
                &workspace_path,
                CodexIssueContext {
                    issue_id,
                    title: issue_title,
                },
                prompt,
                max_turns,
                should_continue,
                on_event,
            )
            .await
    }
}
