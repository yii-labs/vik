use std::future::Future;
use std::path::Path;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::mpsc;
use vik_core::{AgentEvent, AgentRunRequest, AgentSession, IssueTracker, normalize_state};
use vik_workflow::{CodexConfig, ServiceConfig, render_prompt};
use vik_workspace::WorkspaceManager;

use crate::codex::command::codex_spawn_process_command;
use crate::codex::events::agent_event;
use crate::codex::process::SessionLogContext;
use crate::codex::tools::DynamicTools;
use crate::codex::transport::{CodexTransportFactory, ProcessTransportFactory};
use crate::error::AgentError;
use crate::runtime::AgentRuntime;

mod command;
mod events;
mod process;
mod tools;
mod transport;

#[cfg(test)]
mod tests;

const CONTINUATION_PROMPT: &str = "Continue working on this tracker issue. Check current issue state and proceed only if it is still active.";

pub(crate) struct Codex<T>
where
    T: IssueTracker,
{
    tracker: Arc<T>,
    transport_factory: Arc<dyn CodexTransportFactory>,
}

impl<T> Clone for Codex<T>
where
    T: IssueTracker,
{
    fn clone(&self) -> Self {
        Self {
            tracker: Arc::clone(&self.tracker),
            transport_factory: Arc::clone(&self.transport_factory),
        }
    }
}

#[derive(Debug, Clone)]
struct CodexIssue {
    issue_id: String,
    issue_identifier: String,
    title: String,
}

struct RunTurnsInput<'a> {
    workspace_path: &'a Path,
    issue: CodexIssue,
    first_prompt: String,
    max_turns: u32,
    config: &'a CodexConfig,
    tools: DynamicTools,
}

impl<T> Codex<T>
where
    T: IssueTracker,
{
    pub(crate) fn new(tracker: Arc<T>) -> Self {
        Self {
            tracker,
            transport_factory: Arc::new(ProcessTransportFactory),
        }
    }

    #[cfg(test)]
    fn with_transport_factory(
        tracker: Arc<T>,
        transport_factory: Arc<dyn CodexTransportFactory>,
    ) -> Self {
        Self {
            tracker,
            transport_factory,
        }
    }

    async fn run_inner(
        &self,
        request: AgentRunRequest<ServiceConfig>,
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
        let tracker_tools: Arc<dyn IssueTracker> = self.tracker.clone();
        let tools = DynamicTools::from_tracker(tracker_tools)
            .with_issue_context(&request.issue)
            .with_workspace_root(&workspace.path);
        let active_states = request.config.tracker.active_states().to_vec();
        let terminal_states = request.config.tracker.terminal_states().to_vec();
        let issue_id = request.issue.id.clone();
        let tracker = Arc::clone(&self.tracker);
        let result = self
            .run_turns(
                RunTurnsInput {
                    workspace_path: &workspace.path,
                    issue: CodexIssue {
                        issue_id: request.issue.id.clone(),
                        issue_identifier: request.issue.identifier.clone(),
                        title: format!("{}: {}", request.issue.identifier, request.issue.title),
                    },
                    first_prompt: prompt,
                    max_turns: request.config.agent.max_turns,
                    config: &request.config.codex,
                    tools,
                },
                move || {
                    let tracker = Arc::clone(&tracker);
                    let issue_id = issue_id.clone();
                    let active_states = active_states.clone();
                    let terminal_states = terminal_states.clone();
                    async move {
                        let states = tracker
                            .fetch_states_by_ids(std::slice::from_ref(&issue_id))
                            .await?;
                        let Some(issue) = states.first() else {
                            return Ok(false);
                        };
                        let normalized = normalize_state(&issue.state);
                        Ok(active_states
                            .iter()
                            .any(|state| normalize_state(state) == normalized)
                            && !terminal_states
                                .iter()
                                .any(|state| normalize_state(state) == normalized))
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

    async fn run_turns<F, Fut>(
        &self,
        input: RunTurnsInput<'_>,
        mut should_continue: F,
        mut on_event: impl FnMut(AgentEvent) + Send,
    ) -> Result<(), AgentError>
    where
        F: FnMut() -> Fut + Send,
        Fut: Future<Output = Result<bool, AgentError>> + Send,
    {
        let RunTurnsInput {
            workspace_path,
            issue,
            first_prompt,
            max_turns,
            config,
            tools,
        } = input;
        if !workspace_path.is_absolute() {
            return Err(AgentError::InvalidWorkspaceCwd);
        }
        let issue_identifier = issue.issue_identifier.clone();
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            "codex_process_starting",
            json!({}),
        );
        let command = codex_spawn_process_command(config);
        let mut process = self
            .transport_factory
            .spawn(&command, workspace_path, config, tools)
            .await?;
        process.set_session_log_context(SessionLogContext::new(
            issue.issue_id.clone(),
            issue.issue_identifier.clone(),
        ));
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            "codex_process_started",
            json!({ "pid": process.process_id() }),
        );
        emit_lifecycle_event(
            &mut on_event,
            &issue.issue_id,
            "codex_initialize_starting",
            json!({}),
        );
        let mut thread_id_for_cleanup = None;
        let result = async {
            process.initialize().await?;
            emit_lifecycle_event(
                &mut on_event,
                &issue.issue_id,
                "codex_initialize_completed",
                json!({}),
            );
            emit_lifecycle_event(
                &mut on_event,
                &issue.issue_id,
                "codex_thread_starting",
                json!({ "cwd": workspace_path.display().to_string() }),
            );
            let thread_id = process
                .thread_start(workspace_path, &issue.title, config)
                .await?;
            thread_id_for_cleanup = Some(thread_id.clone());
            emit_lifecycle_event(
                &mut on_event,
                &issue.issue_id,
                "codex_thread_started",
                json!({ "thread_id": &thread_id }),
            );
            let mut turn_count = 0_u32;
            loop {
                turn_count += 1;
                let prompt = if turn_count == 1 {
                    first_prompt.clone()
                } else {
                    CONTINUATION_PROMPT.to_string()
                };
                emit_lifecycle_event(
                    &mut on_event,
                    &issue.issue_id,
                    "codex_turn_starting",
                    json!({ "thread_id": &thread_id, "turn_count": turn_count }),
                );
                process.set_session_log_context(SessionLogContext::for_thread(
                    issue.issue_id.clone(),
                    issue_identifier.clone(),
                    thread_id.clone(),
                ));
                let turn_start = process
                    .turn_start(&thread_id, workspace_path, prompt, config)
                    .await?;
                let turn_id = turn_start.turn_id;
                let mut live = AgentSession::new(thread_id.clone(), turn_id.clone());
                live.turn_count = turn_count;
                live.process_id = process.process_id();
                process.set_session_log_context(SessionLogContext::for_session(
                    issue.issue_id.clone(),
                    issue_identifier.clone(),
                    thread_id.clone(),
                    turn_id.clone(),
                ));
                on_event(agent_event(
                    issue.issue_id.clone(),
                    "session_started",
                    Some(live.clone()),
                    None,
                    None,
                    json!({ "thread_id": thread_id, "turn_id": turn_id }),
                ));
                process
                    .wait_for_turn(
                        &thread_id,
                        &turn_id,
                        &mut live,
                        &issue.issue_id,
                        &mut on_event,
                    )
                    .await?;
                if turn_count >= max_turns || !should_continue().await? {
                    break Ok(());
                }
            }
        }
        .await;
        if let Some(thread_id) = thread_id_for_cleanup.as_deref() {
            process.unsubscribe(thread_id).await;
        }
        process.shutdown().await;
        result
    }
}

#[async_trait]
impl<T> AgentRuntime for Codex<T>
where
    T: IssueTracker,
{
    async fn run(
        &self,
        request: AgentRunRequest<ServiceConfig>,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<(), AgentError> {
        self.run_inner(request, events).await
    }
}

#[cfg(test)]
pub(crate) fn message_belongs_to_turn(message: &serde_json::Value, turn_id: &str) -> bool {
    message_turn_id(message).is_none_or(|message_turn_id| message_turn_id == turn_id)
}

#[cfg(test)]
fn message_turn_id(message: &serde_json::Value) -> Option<&str> {
    message
        .pointer("/params/turn/id")
        .or_else(|| message.pointer("/params/turnId"))
        .or_else(|| message.pointer("/result/turn/id"))
        .and_then(serde_json::Value::as_str)
}

fn emit_lifecycle_event(
    on_event: &mut impl FnMut(AgentEvent),
    issue_id: &str,
    event: &'static str,
    raw: serde_json::Value,
) {
    on_event(agent_event(
        issue_id.to_string(),
        event,
        None,
        None,
        None,
        raw,
    ));
}
