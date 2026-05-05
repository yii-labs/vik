use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::thread;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::{mpsc, oneshot};
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

struct SessionRun<T>
where
    T: IssueTracker,
{
    codex: Codex<T>,
    workspace_path: PathBuf,
    issue: CodexIssue,
    first_prompt: String,
    max_turns: u32,
    config: CodexConfig,
    tools: DynamicTools,
    active_states: Vec<String>,
    terminal_states: Vec<String>,
    issue_id: String,
    tracker: Arc<T>,
    events: mpsc::UnboundedSender<AgentEvent>,
}

async fn run_session_on_thread<T>(run: SessionRun<T>) -> Result<(), AgentError>
where
    T: IssueTracker,
{
    let thread_name = session_thread_name(&run.issue.issue_identifier);
    let (result_tx, result_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let _cancel_on_drop = SessionCancelOnDrop::new(cancel_tx);
    let handle = thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let result = run_session_in_current_thread(run, cancel_rx);
            let _ = result_tx.send(result);
        })
        .map_err(|err| AgentError::SessionThread(err.to_string()))?;
    let result = result_rx
        .await
        .map_err(|_| AgentError::SessionThread("session thread exited without result".into()))?;
    handle
        .join()
        .map_err(|_| AgentError::SessionThread("session thread panicked".into()))?;
    result
}

fn run_session_in_current_thread<T>(
    run: SessionRun<T>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<(), AgentError>
where
    T: IssueTracker,
{
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|err| AgentError::SessionThread(err.to_string()))?;
    runtime.block_on(async move {
        tokio::select! {
            result = run_session(run) => result,
            _ = cancel_rx => Err(AgentError::TurnCancelled),
        }
    })
}

async fn run_session<T>(run: SessionRun<T>) -> Result<(), AgentError>
where
    T: IssueTracker,
{
    let SessionRun {
        codex,
        workspace_path,
        issue,
        first_prompt,
        max_turns,
        config,
        tools,
        active_states,
        terminal_states,
        issue_id,
        tracker,
        events,
    } = run;
    codex
        .run_turns(
            RunTurnsInput {
                workspace_path: &workspace_path,
                issue,
                first_prompt,
                max_turns,
                config: &config,
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
        .await
}

struct SessionCancelOnDrop {
    tx: Option<oneshot::Sender<()>>,
}

impl SessionCancelOnDrop {
    fn new(tx: oneshot::Sender<()>) -> Self {
        Self { tx: Some(tx) }
    }
}

impl Drop for SessionCancelOnDrop {
    fn drop(&mut self) {
        if let Some(tx) = self.tx.take() {
            let _ = tx.send(());
        }
    }
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
        let result = run_session_on_thread(SessionRun {
            codex: self.clone(),
            workspace_path: workspace.path.clone(),
            issue: CodexIssue {
                issue_id: request.issue.id.clone(),
                issue_identifier: request.issue.identifier.clone(),
                title: format!("{}: {}", request.issue.identifier, request.issue.title),
            },
            first_prompt: prompt,
            max_turns: request.config.agent.max_turns,
            config: request.config.codex.clone(),
            tools,
            active_states,
            terminal_states,
            issue_id,
            tracker,
            events,
        })
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

pub(crate) fn session_thread_name(issue_identifier: &str) -> String {
    let sanitized: String = issue_identifier
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '.' || ch == '_' || ch == '-' {
                ch
            } else {
                '_'
            }
        })
        .collect();
    let suffix = if sanitized.is_empty() {
        "unknown".to_string()
    } else {
        sanitized
    };
    format!("vik-session-{suffix}")
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
