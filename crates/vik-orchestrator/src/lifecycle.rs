use std::sync::Arc;

use chrono::Utc;
use vik_core::{
    AgentRunRequest, AgentWorker, Issue, IssueTracker, TokenUsage, sanitize_workspace_key,
};
use vik_workflow::{LoadedWorkflow, ServiceConfig};
use vik_workspace::WorkspaceManager;

use crate::engine::Orchestrator;
use crate::state::RunningEntry;

impl<T, W> Orchestrator<T, W>
where
    T: IssueTracker,
    W: AgentWorker<ServiceConfig>,
{
    pub(crate) async fn startup_cleanup(&self) {
        let loaded = self.current_loaded().await;
        let terminal_states = loaded.config.tracker.terminal_states.clone();
        match self.tracker.fetch_issues_by_states(&terminal_states).await {
            Ok(issues) => {
                let manager =
                    WorkspaceManager::new(loaded.config.workspace.root, loaded.config.hooks);
                for issue in issues {
                    if let Err(err) = manager.remove_for_issue(&issue.identifier).await {
                        tracing::warn!(
                            issue_id=%issue.id,
                            issue_identifier=%issue.identifier,
                            error=%err,
                            "startup_cleanup outcome=failed"
                        );
                    }
                }
            }
            Err(err) => {
                tracing::warn!(error=%err, "startup_cleanup_terminal_fetch outcome=failed");
            }
        }
    }

    pub(crate) async fn dispatch_issue(
        &self,
        issue: Issue,
        attempt: Option<u32>,
        loaded: LoadedWorkflow,
    ) {
        tracing::info!(
            issue_id=%issue.id,
            issue_identifier=%issue.identifier,
            attempt=?attempt,
            "dispatch outcome=started"
        );
        let workspace_path = loaded
            .config
            .workspace
            .root
            .join(sanitize_workspace_key(&issue.identifier));
        let request = AgentRunRequest {
            issue: issue.clone(),
            attempt,
            workflow: loaded.definition,
            config: loaded.config,
        };
        let worker = Arc::clone(&self.worker);
        let events = self.event_tx.clone();
        let outcome_tx = self.outcome_tx.clone();
        let issue_id = issue.id.clone();
        let issue_identifier = issue.identifier.clone();
        let handle = tokio::spawn(async move {
            let outcome = worker.run(request, events).await;
            let _ = outcome_tx.send(outcome.clone());
            outcome
        });
        let mut state = self.state.lock().await;
        state
            .issue_identifiers
            .insert(issue_id.clone(), issue_identifier.clone());
        state.claimed.insert(issue_id.clone());
        state.retry_attempts.remove(&issue_id);
        state.running.insert(
            issue_id,
            RunningEntry {
                issue,
                identifier: issue_identifier,
                retry_attempt: attempt,
                started_at: Utc::now(),
                workspace_path: Some(workspace_path),
                session_id: None,
                turn_count: 0,
                last_event: None,
                last_message: None,
                last_event_at: None,
                tokens: TokenUsage::default(),
                last_reported_input_tokens: 0,
                last_reported_output_tokens: 0,
                last_reported_total_tokens: 0,
                abort: handle,
            },
        );
    }
}
