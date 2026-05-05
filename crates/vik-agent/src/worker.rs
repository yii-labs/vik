use std::sync::Arc;
use std::thread;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use vik_core::{
    AgentEvent, AgentRunRequest, AgentWorker, IssueTracker, WorkerOutcome, sanitize_workspace_key,
};
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
        );
        let workspace = manager.create_for_issue(&request.issue.identifier).await?;
        manager.validate_agent_cwd(&workspace.path, &workspace.path)?;
        manager.before_run(&workspace.path).await?;
        let prompt = render_prompt(&request.workflow, &request.issue, request.attempt)?;
        let active_states = request.config.tracker.active_states.clone();
        let terminal_states = request.config.tracker.terminal_states.clone();
        let issue_id = request.issue.id.clone();
        let tracker = Arc::clone(&self.tracker);
        let codex_config = request.config.codex.clone();
        let tracker_config = request.config.tracker.clone();
        let issue_identifier = request.issue.identifier.clone();
        let result = run_session_on_thread(SessionRun {
            workspace_path: workspace.path.clone(),
            issue: CodexIssueContext {
                issue_id: request.issue.id.clone(),
                title: format!("{}: {}", request.issue.identifier, request.issue.title),
            },
            prompt,
            max_turns: request.config.agent.max_turns,
            codex_config,
            tracker_config,
            active_states,
            terminal_states,
            issue_id,
            issue_identifier,
            tracker,
            events,
        })
        .await;
        manager.after_run_best_effort(&workspace.path).await;
        result
    }
}

struct SessionRun<T>
where
    T: IssueTracker,
{
    workspace_path: std::path::PathBuf,
    issue: CodexIssueContext,
    prompt: String,
    max_turns: u32,
    codex_config: vik_workflow::CodexConfig,
    tracker_config: vik_workflow::TrackerConfig,
    active_states: Vec<String>,
    terminal_states: Vec<String>,
    issue_id: String,
    issue_identifier: String,
    tracker: Arc<T>,
    events: mpsc::UnboundedSender<AgentEvent>,
}

async fn run_session_on_thread<T>(run: SessionRun<T>) -> Result<(), AgentError>
where
    T: IssueTracker,
{
    let thread_name = session_thread_name(&run.issue_identifier);
    let (tx, rx) = oneshot::channel();
    thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let result = run_session_in_current_thread(run);
            let _ = tx.send(result);
        })
        .map_err(|err| AgentError::SessionThread(err.to_string()))?;
    rx.await
        .map_err(|_| AgentError::SessionThread("session thread exited without result".into()))?
}

fn run_session_in_current_thread<T>(run: SessionRun<T>) -> Result<(), AgentError>
where
    T: IssueTracker,
{
    let SessionRun {
        workspace_path,
        issue,
        prompt,
        max_turns,
        codex_config,
        tracker_config,
        active_states,
        terminal_states,
        issue_id,
        tracker,
        events,
        ..
    } = run;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| AgentError::SessionThread(err.to_string()))?;
    runtime.block_on(async move {
        let tools = DynamicTools::from_tracker_config(&tracker_config);
        let client = CodexAppServerClient::new(codex_config).with_dynamic_tools(tools);
        client
            .run_turns(
                &workspace_path,
                issue,
                prompt,
                max_turns,
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
            .await
    })
}

pub(crate) fn session_thread_name(issue_identifier: &str) -> String {
    format!("vik-session-{}", sanitize_workspace_key(issue_identifier))
}
