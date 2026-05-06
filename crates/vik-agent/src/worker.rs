use std::sync::Arc;
use std::thread;

use async_trait::async_trait;
use tokio::sync::{mpsc, oneshot};
use vik_core::{AgentEvent, AgentRunRequest, AgentWorker, IssueTracker, WorkerOutcome};
use vik_workflow::{AgentRuntimeConfig, ServiceConfig};

use crate::codex::Codex;
use crate::error::AgentError;
use crate::runtime::AgentRuntime;
use crate::session_log::with_session_log_subscriber;

#[derive(Clone)]
pub struct LocalAgentWorker<T>
where
    T: IssueTracker,
{
    tracker: Arc<T>,
    #[cfg(test)]
    runtime_override: Option<Arc<dyn AgentRuntime>>,
}

impl<T> LocalAgentWorker<T>
where
    T: IssueTracker,
{
    pub fn new(tracker: Arc<T>) -> Self {
        Self {
            tracker,
            #[cfg(test)]
            runtime_override: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn with_runtime_override(tracker: Arc<T>, runtime: Arc<dyn AgentRuntime>) -> Self {
        Self {
            tracker,
            runtime_override: Some(runtime),
        }
    }

    fn runtime_for(&self, runtime: AgentRuntimeConfig) -> Arc<dyn AgentRuntime> {
        #[cfg(test)]
        if let Some(runtime) = &self.runtime_override {
            return Arc::clone(runtime);
        }

        match runtime {
            AgentRuntimeConfig::Codex => Arc::new(Codex::new(Arc::clone(&self.tracker))),
        }
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
        let issue = request.issue.clone();
        let runtime = self.runtime_for(request.config.agent.runtime);
        match run_runtime_on_session_thread(runtime, request, events).await {
            Ok(()) => WorkerOutcome::normal(&issue),
            Err(err) => WorkerOutcome::failed(&issue, err.to_string()),
        }
    }
}

async fn run_runtime_on_session_thread(
    runtime: Arc<dyn AgentRuntime>,
    request: AgentRunRequest<ServiceConfig>,
    events: mpsc::UnboundedSender<AgentEvent>,
) -> Result<(), AgentError> {
    let thread_name = session_thread_name(&request.issue.identifier);
    let (result_tx, result_rx) = oneshot::channel();
    let (cancel_tx, cancel_rx) = oneshot::channel();
    let _cancel_on_drop = SessionCancelOnDrop::new(cancel_tx);
    let handle = thread::Builder::new()
        .name(thread_name)
        .spawn(move || {
            let result = run_runtime_in_current_thread(runtime, request, events, cancel_rx);
            let _ = result_tx.send(result);
        })
        .map_err(|err| AgentError::SessionThread(err.to_string()))?;
    let result = match result_rx.await {
        Ok(result) => result,
        Err(_) => {
            return match handle.join() {
                Ok(()) => Err(AgentError::SessionThread(
                    "session thread exited without result".into(),
                )),
                Err(_) => Err(AgentError::SessionThread("session thread panicked".into())),
            };
        }
    };
    handle
        .join()
        .map_err(|_| AgentError::SessionThread("session thread panicked".into()))?;
    result
}

fn run_runtime_in_current_thread(
    runtime: Arc<dyn AgentRuntime>,
    request: AgentRunRequest<ServiceConfig>,
    events: mpsc::UnboundedSender<AgentEvent>,
    cancel_rx: oneshot::Receiver<()>,
) -> Result<(), AgentError> {
    let log_dir = request.config.logging.dir.clone();
    with_session_log_subscriber(&log_dir, || {
        let runtime_loop = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .map_err(|err| AgentError::SessionThread(err.to_string()))?;
        runtime_loop.block_on(async move {
            tokio::select! {
                result = runtime.run(request, events) => result,
                _ = cancel_rx => Err(AgentError::TurnCancelled),
            }
        })
    })
    .map_err(|err| AgentError::SessionThread(format!("session logging: {err}")))?
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
