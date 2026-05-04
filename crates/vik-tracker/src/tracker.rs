use async_trait::async_trait;
use vik_core::{Issue, IssueTracker, TrackerError};

pub struct TrackerClient {
    inner: Box<dyn IssueTracker>,
}

impl TrackerClient {
    pub fn new<T>(inner: T) -> Self
    where
        T: IssueTracker,
    {
        Self {
            inner: Box::new(inner),
        }
    }
}

#[async_trait]
impl IssueTracker for TrackerClient {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_candidate_issues().await
    }

    async fn fetch_issues_by_states(
        &self,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_issues_by_states(state_names).await
    }

    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_issue_states_by_ids(issue_ids).await
    }
}
