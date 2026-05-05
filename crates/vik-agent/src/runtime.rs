use async_trait::async_trait;
use tokio::sync::mpsc;
use vik_core::{AgentEvent, AgentRunRequest};
use vik_workflow::ServiceConfig;

use crate::error::AgentError;

#[async_trait]
pub trait AgentRuntime: Send + Sync + 'static {
    async fn run(
        &self,
        request: AgentRunRequest<ServiceConfig>,
        events: mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<(), AgentError>;
}
