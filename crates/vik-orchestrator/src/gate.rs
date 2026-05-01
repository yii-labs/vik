use async_trait::async_trait;
use vik_core::Issue;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DispatchDecision {
    Allow,
    Block(String),
}

#[async_trait]
pub trait DispatchGate: Send + Sync + 'static {
    async fn should_dispatch(&self, issue: &Issue) -> DispatchDecision;
}
