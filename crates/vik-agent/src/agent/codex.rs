use async_trait::async_trait;
use vik_workflow::CodingAgentKind;

use super::{CodingAgentAdapter, CodingAgentRun};
use crate::client::{CodexAppServerClient, CodexIssueContext};
use crate::error::AgentError;

pub(crate) struct CodexAdapter {
    pub(crate) client: CodexAppServerClient,
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
