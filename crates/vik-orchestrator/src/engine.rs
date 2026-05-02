use std::sync::Arc;
use std::time::Duration;

use chrono::Utc;
use tokio::sync::{Mutex, mpsc};
use tokio::time;
use vik_core::{AgentEvent, AgentWorker, IssueTracker, WorkerOutcome};
use vik_workflow::{LoadedWorkflow, ServiceConfig, WorkflowReloader};

use crate::dispatch::{should_dispatch, sort_for_dispatch};
use crate::error::OrchestratorError;
use crate::gate::{DispatchDecision, DispatchGate};
use crate::session_log::{
    append_session_log_blocking, attach_session_logs, issue_debug_from_session_logs,
    read_session_logs_blocking,
};
use crate::state::OrchestratorState;

pub struct Orchestrator<T, W>
where
    T: IssueTracker,
    W: AgentWorker<ServiceConfig>,
{
    pub(crate) tracker: Arc<T>,
    pub(crate) worker: Arc<W>,
    pub(crate) reloader: Mutex<WorkflowReloader>,
    pub(crate) state: Arc<Mutex<OrchestratorState>>,
    pub(crate) event_tx: mpsc::UnboundedSender<AgentEvent>,
    pub(crate) event_rx: Mutex<mpsc::UnboundedReceiver<AgentEvent>>,
    pub(crate) outcome_tx: mpsc::UnboundedSender<WorkerOutcome>,
    pub(crate) outcome_rx: Mutex<mpsc::UnboundedReceiver<WorkerOutcome>>,
    pub(crate) refresh_tx: mpsc::UnboundedSender<()>,
    pub(crate) refresh_rx: Mutex<mpsc::UnboundedReceiver<()>>,
    pub(crate) dispatch_gate: Mutex<Option<Arc<dyn DispatchGate>>>,
}

impl<T, W> Orchestrator<T, W>
where
    T: IssueTracker,
    W: AgentWorker<ServiceConfig>,
{
    pub fn new(tracker: Arc<T>, worker: Arc<W>, reloader: WorkflowReloader) -> Self {
        let config = &reloader.current().config;
        let state = Arc::new(Mutex::new(OrchestratorState::new(config)));
        let (event_tx, event_rx) = mpsc::unbounded_channel();
        let (outcome_tx, outcome_rx) = mpsc::unbounded_channel();
        let (refresh_tx, refresh_rx) = mpsc::unbounded_channel();
        Self {
            tracker,
            worker,
            reloader: Mutex::new(reloader),
            state,
            event_tx,
            event_rx: Mutex::new(event_rx),
            outcome_tx,
            outcome_rx: Mutex::new(outcome_rx),
            refresh_tx,
            refresh_rx: Mutex::new(refresh_rx),
            dispatch_gate: Mutex::new(None),
        }
    }

    pub async fn set_dispatch_gate(&self, gate: Arc<dyn DispatchGate>) {
        *self.dispatch_gate.lock().await = Some(gate);
    }

    pub async fn snapshot(&self) -> vik_core::RuntimeSnapshot {
        self.state.lock().await.snapshot()
    }

    pub async fn issue_debug(
        &self,
        issue_identifier: &str,
    ) -> Option<vik_core::IssueDebugSnapshot> {
        let snapshot = self.state.lock().await.issue_debug(issue_identifier);
        let logging_dir = self.current_loaded().await.config.logging.dir;
        let logs =
            match read_session_logs_blocking(logging_dir, issue_identifier.to_string(), 50).await {
                Ok(logs) => logs,
                Err(err) => {
                    tracing::warn!(
                        issue_identifier,
                        error=%err,
                        "session_log_read outcome=failed"
                    );
                    Vec::new()
                }
            };
        match snapshot {
            Some(snapshot) => Some(attach_session_logs(snapshot, logs)),
            None => issue_debug_from_session_logs(issue_identifier, logs),
        }
    }

    pub fn refresh_sender(&self) -> mpsc::UnboundedSender<()> {
        self.refresh_tx.clone()
    }

    pub async fn run_forever(&self) -> Result<(), OrchestratorError> {
        self.startup_cleanup().await;
        let mut poll_interval_ms = {
            let state = self.state.lock().await;
            state.poll_interval_ms
        };
        let mut interval = time::interval(Duration::from_millis(poll_interval_ms));
        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                _ = interval.tick() => {
                    self.tick().await;
                    let ms = self.state.lock().await.poll_interval_ms;
                    if ms != poll_interval_ms {
                        poll_interval_ms = ms;
                        interval = time::interval(Duration::from_millis(poll_interval_ms));
                        interval.set_missed_tick_behavior(time::MissedTickBehavior::Delay);
                        interval.tick().await;
                    }
                }
                Some(event) = self.next_agent_event() => {
                    let log_entry = self.state.lock().await.apply_agent_event(event);
                    let logging_dir = self.current_loaded().await.config.logging.dir;
                    let issue_id = log_entry.issue_id.clone();
                    let issue_identifier = log_entry.issue_identifier.clone();
                    if let Err(err) = append_session_log_blocking(logging_dir, log_entry).await {
                        tracing::warn!(
                            issue_id=%issue_id,
                            issue_identifier=%issue_identifier,
                            error=%err,
                            "session_log_append outcome=failed"
                        );
                    }
                }
                Some(outcome) = self.next_worker_outcome() => {
                    let config = self.current_loaded().await.config;
                    self.state.lock().await.on_worker_exit(outcome, &config);
                }
                Some(()) = self.next_refresh() => {
                    self.tick().await;
                }
                () = self.sleep_until_next_retry() => {
                    if let Some(loaded) = self.current_loaded_for_dispatch().await {
                        self.process_due_retries(loaded).await;
                    }
                }
            }
        }
    }

    async fn next_agent_event(&self) -> Option<AgentEvent> {
        self.event_rx.lock().await.recv().await
    }

    async fn next_worker_outcome(&self) -> Option<WorkerOutcome> {
        self.outcome_rx.lock().await.recv().await
    }

    async fn next_refresh(&self) -> Option<()> {
        self.refresh_rx.lock().await.recv().await
    }

    async fn sleep_until_next_retry(&self) {
        let delay = {
            let state = self.state.lock().await;
            state
                .retry_attempts
                .values()
                .map(|entry| entry.due_at)
                .min()
                .map(|due_at| {
                    let millis = (due_at - Utc::now()).num_milliseconds().max(0) as u64;
                    Duration::from_millis(millis)
                })
        };
        match delay {
            Some(delay) => time::sleep(delay).await,
            None => std::future::pending::<()>().await,
        }
    }

    pub async fn tick(&self) {
        self.reconcile_running_issues().await;
        let Some(loaded) = self.current_loaded_for_dispatch().await else {
            return;
        };
        self.state.lock().await.apply_config(&loaded.config);
        if let Err(err) = loaded.config.validate_for_dispatch() {
            tracing::error!(error=%err, "dispatch_preflight outcome=failed");
            return;
        }
        let issues = match self.tracker.fetch_candidate_issues().await {
            Ok(issues) => issues,
            Err(err) => {
                tracing::error!(error=%err, "tracker_fetch_candidates outcome=failed");
                return;
            }
        };
        tracing::info!(
            candidate_count = issues.len(),
            "tracker_fetch_candidates outcome=ok"
        );
        let sorted = sort_for_dispatch(issues);
        for issue in sorted {
            let should_dispatch = {
                let state = self.state.lock().await;
                should_dispatch(&issue, &state, &loaded.config)
            };
            if !should_dispatch {
                continue;
            }
            match self.dispatch_decision(&issue).await {
                DispatchDecision::Allow => {
                    self.dispatch_issue(issue, None, loaded.clone()).await;
                }
                DispatchDecision::Block(reason) => {
                    tracing::info!(
                        issue_id=%issue.id,
                        issue_identifier=%issue.identifier,
                        reason=%reason,
                        "dispatch outcome=gated"
                    );
                }
            }
        }
        self.process_due_retries(loaded).await;
    }

    pub(crate) async fn dispatch_decision(&self, issue: &vik_core::Issue) -> DispatchDecision {
        let gate = self.dispatch_gate.lock().await.clone();
        match gate {
            Some(gate) => gate.should_dispatch(issue).await,
            None => DispatchDecision::Allow,
        }
    }

    pub(crate) async fn current_loaded(&self) -> LoadedWorkflow {
        let mut reloader = self.reloader.lock().await;
        if let Err(err) = reloader.reload_if_changed() {
            tracing::error!(error=%err, "workflow_reload outcome=failed keeping_last_good=true");
        }
        reloader.current().clone()
    }

    pub(crate) async fn current_loaded_for_dispatch(&self) -> Option<LoadedWorkflow> {
        let mut reloader = self.reloader.lock().await;
        if let Err(err) = reloader.reload_if_changed() {
            tracing::error!(
                error=%err,
                "workflow_reload outcome=failed dispatch_blocked=true keeping_last_good_for_reconciliation=true"
            );
            return None;
        }
        Some(reloader.current().clone())
    }
}
