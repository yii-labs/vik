use async_trait::async_trait;
use thiserror::Error;

use crate::Issue;

#[derive(Debug, Error)]
pub enum TrackerError {
    #[error("unsupported_tracker_kind")]
    UnsupportedTrackerKind,
    #[error("missing_tracker_api_key")]
    MissingTrackerApiKey,
    #[error("missing_tracker_project_slug")]
    MissingTrackerProjectSlug,
    #[error("linear_api_request: {0}")]
    LinearApiRequest(String),
    #[error("linear_api_status: {0}")]
    LinearApiStatus(u16),
    #[error("linear_graphql_errors: {0}")]
    LinearGraphqlErrors(String),
    #[error("linear_unknown_payload: {0}")]
    LinearUnknownPayload(String),
    #[error("linear_missing_end_cursor")]
    LinearMissingEndCursor,
}

#[async_trait]
pub trait IssueTracker: Send + Sync + 'static {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError>;
    async fn fetch_issues_by_states(
        &self,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError>;
    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError>;
}
