use async_trait::async_trait;
use vik_core::{Issue, IssueTracker, TrackerError};

use crate::{GitHubClient, LinearClient};

#[derive(Debug, Clone)]
pub enum TrackerClient {
    Linear(LinearClient),
    GitHub(GitHubClient),
}

#[async_trait]
impl IssueTracker for TrackerClient {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        match self {
            Self::Linear(client) => client.fetch_candidate_issues().await,
            Self::GitHub(client) => client.fetch_candidate_issues().await,
        }
    }

    async fn fetch_issues_by_states(
        &self,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        match self {
            Self::Linear(client) => client.fetch_issues_by_states(state_names).await,
            Self::GitHub(client) => client.fetch_issues_by_states(state_names).await,
        }
    }

    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        match self {
            Self::Linear(client) => client.fetch_issue_states_by_ids(issue_ids).await,
            Self::GitHub(client) => client.fetch_issue_states_by_ids(issue_ids).await,
        }
    }
}
