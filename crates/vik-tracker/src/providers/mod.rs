use std::path::Path;

use async_trait::async_trait;
use vik_core::{Issue, IssueTracker, TrackerError};

pub mod github;
pub mod linear;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IssueUpdate {
    pub state: Option<String>,
    pub labels: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueComment {
    pub id: String,
    pub body: String,
    pub url: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssueAttachment {
    pub url: String,
    pub comment: Option<IssueComment>,
}

#[async_trait]
pub trait Tracker: Send + Sync + 'static {
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

pub struct TrackerClient {
    inner: Box<dyn Tracker>,
}

impl TrackerClient {
    pub fn new(inner: Box<dyn Tracker>) -> Self {
        Self { inner }
    }
}

impl std::fmt::Debug for TrackerClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrackerClient").finish_non_exhaustive()
    }
}

#[async_trait]
impl Tracker for TrackerClient {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_candidates().await
    }

    async fn fetch_by_states(&self, state_names: &[String]) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_by_states(state_names).await
    }

    async fn fetch_states_by_ids(&self, issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        self.inner.fetch_states_by_ids(issue_ids).await
    }

    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError> {
        self.inner.get_issue(issue_id).await
    }

    async fn update_issue(
        &self,
        issue_id: &str,
        update: IssueUpdate,
    ) -> Result<Issue, TrackerError> {
        self.inner.update_issue(issue_id, update).await
    }

    async fn create_comment(
        &self,
        issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        self.inner.create_comment(issue_id, body).await
    }

    async fn update_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        self.inner.update_comment(comment_id, body).await
    }

    async fn upload_attachment(
        &self,
        issue_id: &str,
        path: &Path,
        content_type: &str,
    ) -> Result<IssueAttachment, TrackerError> {
        self.inner
            .upload_attachment(issue_id, path, content_type)
            .await
    }

    async fn link_pr(&self, issue_id: &str, title: &str, url: &str) -> Result<(), TrackerError> {
        self.inner.link_pr(issue_id, title, url).await
    }
}

#[async_trait]
impl IssueTracker for TrackerClient {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_candidates().await
    }

    async fn fetch_issues_by_states(
        &self,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_by_states(state_names).await
    }

    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_states_by_ids(issue_ids).await
    }
}
