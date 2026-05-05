use std::sync::Arc;

use async_trait::async_trait;
use tokio::sync::mpsc;
use vik_core::{AgentEvent, AgentRunRequest, AgentWorker, IssueTracker, WorkerOutcome};
use vik_workflow::{AgentRuntimeConfig, ServiceConfig};

use crate::codex::Codex;
use crate::runtime::AgentRuntime;

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
        match runtime.run(request, events).await {
            Ok(()) => WorkerOutcome::normal(&issue),
            Err(err) => WorkerOutcome::failed(&issue, err.to_string()),
        }
    }
}
