use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use std::path::Path;

use crate::Issue;

#[derive(Debug, Error)]
pub enum TrackerError {
    #[error("unsupported_tracker_kind")]
    UnsupportedTrackerKind,
    #[error("missing_tracker_api_key")]
    MissingTrackerApiKey,
    #[error("missing_tracker_project_slug")]
    MissingTrackerProjectSlug,
    #[error("missing_tracker_repository")]
    MissingTrackerRepository,
    #[error("invalid_tracker_repository: {0}")]
    InvalidTrackerRepository(String),
    #[error("unsupported_tracker_operation: {0}")]
    UnsupportedTrackerOperation(String),
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
    #[error("github_api_request: {0}")]
    GitHubApiRequest(String),
    #[error("github_api_status: {0}")]
    GitHubApiStatus(u16),
    #[error("github_unknown_payload: {0}")]
    GitHubUnknownPayload(String),
}

#[async_trait]
pub trait IssueTracker: Send + Sync + 'static {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError>;
    async fn fetch_by_states(&self, state_names: &[String]) -> Result<Vec<Issue>, TrackerError>;
    async fn fetch_states_by_ids(&self, issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError>;
    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError>;
    async fn update_issue(
        &self,
        issue_id: &str,
        update: IssueUpdate,
    ) -> Result<Issue, TrackerError>;
    async fn create_comment(
        &self,
        issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError>;
    async fn list_comments(&self, issue_id: &str) -> Result<Vec<IssueComment>, TrackerError>;
    async fn update_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError>;
    async fn upload_attachment(
        &self,
        issue_id: &str,
        path: &Path,
        content_type: &str,
    ) -> Result<IssueAttachment, TrackerError>;
    async fn link_pr(&self, issue_id: &str, title: &str, url: &str) -> Result<(), TrackerError>;
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueUpdate {
    pub state: Option<String>,
    #[serde(default)]
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueComment {
    pub id: String,
    pub body: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IssueAttachment {
    pub url: String,
    pub comment: Option<IssueComment>,
}
