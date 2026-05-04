use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use vik_core::{AgentEvent, AgentRunRequest, AgentWorker, IssueTracker, WorkerOutcome};
use vik_workflow::{ServiceConfig, render_prompt};
use vik_workspace::WorkspaceManager;

use crate::client::{CodexAppServerClient, CodexIssueContext};
use crate::error::AgentError;
use crate::tools::DynamicTools;

#[derive(Clone)]
pub struct LocalAgentWorker<T>
where
    T: IssueTracker,
{
    tracker: Arc<T>,
}

impl<T> LocalAgentWorker<T>
where
    T: IssueTracker,
{
    pub fn new(tracker: Arc<T>) -> Self {
        Self { tracker }
    }
}

#[async_trait]
impl<T> AgentWorker<ServiceConfig> for LocalAgentWorker<T>
where
    T: IssueTracker,
{
    async fn run(
        &self,
        request: AgentRunRequest<ServiceConfig>,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> WorkerOutcome {
        match self.run_inner(&request, events).await {
            Ok(()) => WorkerOutcome::normal(&request.issue),
            Err(err) => WorkerOutcome::failed(&request.issue, err.to_string()),
        }
    }
}

impl<T> LocalAgentWorker<T>
where
    T: IssueTracker,
{
    async fn run_inner(
        &self,
        request: &AgentRunRequest<ServiceConfig>,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<(), AgentError> {
        let manager = WorkspaceManager::new(
            request.config.workspace.root.clone(),
            request.config.hooks.clone(),
        )
        .with_env(request.config.runtime_env.clone());
        let workspace = manager.create_for_issue(&request.issue.identifier).await?;
        manager.validate_agent_cwd(&workspace.path, &workspace.path)?;
        manager.before_run(&workspace.path).await?;
        let prompt = render_prompt(&request.workflow, &request.issue, request.attempt)?;
        let tools = DynamicTools::from_tracker_config(&request.config.tracker);
        let client = CodexAppServerClient::new(request.config.codex.clone())
            .with_env(request.config.runtime_env.clone())
            .with_dynamic_tools(tools);
        let active_states = request.config.tracker.active_states.clone();
        let terminal_states = request.config.tracker.terminal_states.clone();
        let issue_id = request.issue.id.clone();
        let tracker = Arc::clone(&self.tracker);
        let result = client
            .run_turns(
                &workspace.path,
                CodexIssueContext {
                    issue_id: request.issue.id.clone(),
                    title: format!("{}: {}", request.issue.identifier, request.issue.title),
                },
                prompt,
                request.config.agent.max_turns,
                move || {
                    let tracker = Arc::clone(&tracker);
                    let issue_id = issue_id.clone();
                    let active_states = active_states.clone();
                    let terminal_states = terminal_states.clone();
                    async move {
                        let states = tracker
                            .fetch_issue_states_by_ids(std::slice::from_ref(&issue_id))
                            .await?;
                        let Some(issue) = states.first() else {
                            return Ok(false);
                        };
                        let normalized = issue.state.to_lowercase();
                        Ok(active_states.iter().any(|s| s.to_lowercase() == normalized)
                            && !terminal_states
                                .iter()
                                .any(|s| s.to_lowercase() == normalized))
                    }
                },
                |event| {
                    let _ = events.send(event);
                },
            )
            .await;
        manager.after_run_best_effort(&workspace.path).await;
        result
    }
}
