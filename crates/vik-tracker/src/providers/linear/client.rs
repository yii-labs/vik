use std::collections::HashSet;
use std::path::Path;

use async_trait::async_trait;
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use serde_json::{Map, Value, json};
use vik_core::{Issue, TrackerError};

use crate::providers::{IssueAttachment, IssueComment, IssueUpdate, Tracker};

use super::normalize::normalize_issue;
use super::queries::{
    ATTACHMENT_CREATE_MUTATION, CANDIDATE_QUERY, COMMENT_CREATE_MUTATION, COMMENT_UPDATE_MUTATION,
    FILE_UPLOAD_MUTATION, ISSUE_BY_ID_QUERY, ISSUE_STATES_BY_IDS_QUERY,
    ISSUE_STATES_FOR_ISSUE_QUERY, ISSUE_UPDATE_MUTATION, ISSUES_BY_STATES_QUERY,
};

pub const DEFAULT_LINEAR_ENDPOINT: &str = "https://api.linear.app/graphql";
pub const DEFAULT_PAGE_SIZE: i64 = 50;

#[derive(Debug, Clone)]
pub struct LinearClientConfig {
    pub endpoint: String,
    pub api_key: String,
    pub project_slug: String,
    pub active_states: Vec<String>,
    pub filter: LinearIssueFilterConfig,
    pub page_size: i64,
}

impl LinearClientConfig {
    pub fn new(
        endpoint: impl Into<String>,
        api_key: impl Into<String>,
        project_slug: impl Into<String>,
        active_states: Vec<String>,
    ) -> Self {
        Self {
            endpoint: endpoint.into(),
            api_key: api_key.into(),
            project_slug: project_slug.into(),
            active_states,
            filter: LinearIssueFilterConfig::default(),
            page_size: DEFAULT_PAGE_SIZE,
        }
    }

    pub fn with_filter(mut self, filter: LinearIssueFilterConfig) -> Self {
        self.filter = filter;
        self
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinearIssueFilterConfig {
    pub assignees: Vec<String>,
    pub tags: Vec<String>,
}

impl LinearIssueFilterConfig {
    pub fn new(assignees: Vec<String>, tags: Vec<String>) -> Self {
        Self {
            assignees: clean_filter_values(assignees),
            tags: clean_filter_values(tags),
        }
    }

    pub(crate) fn assignee_filter_value(&self) -> Value {
        let clauses = self
            .assignees
            .iter()
            .flat_map(|assignee| {
                [
                    json!({ "id": { "eq": assignee } }),
                    json!({ "name": { "eqIgnoreCase": assignee } }),
                    json!({ "displayName": { "eqIgnoreCase": assignee } }),
                    json!({ "email": { "eqIgnoreCase": assignee } }),
                ]
            })
            .collect();
        any_filter(clauses)
    }

    pub(crate) fn label_filter_value(&self) -> Value {
        let clauses = self
            .tags
            .iter()
            .map(|tag| json!({ "name": { "eqIgnoreCase": tag } }))
            .collect();
        let filter = any_filter(clauses);
        if filter.as_object().is_some_and(|map| map.is_empty()) {
            filter
        } else {
            json!({ "some": filter })
        }
    }
}

fn clean_filter_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn any_filter(clauses: Vec<Value>) -> Value {
    match clauses.as_slice() {
        [] => json!({}),
        [clause] => clause.clone(),
        _ => json!({ "or": clauses }),
    }
}

#[derive(Debug, Clone)]
pub struct LinearClient {
    http: reqwest::Client,
    config: LinearClientConfig,
}

impl LinearClient {
    pub fn new(config: LinearClientConfig) -> Result<Self, TrackerError> {
        if config.api_key.trim().is_empty() {
            return Err(TrackerError::MissingTrackerApiKey);
        }
        if config.project_slug.trim().is_empty() {
            return Err(TrackerError::MissingTrackerProjectSlug);
        }
        let http = reqwest::Client::builder()
            .timeout(std::time::Duration::from_millis(30_000))
            .build()
            .map_err(|err| TrackerError::LinearApiRequest(err.to_string()))?;
        Ok(Self { http, config })
    }

    pub async fn graphql(&self, query: &str, variables: Value) -> Result<Value, TrackerError> {
        let body = json!({ "query": query, "variables": variables });
        let response = self
            .http
            .post(&self.config.endpoint)
            .header("Authorization", &self.config.api_key)
            .json(&body)
            .send()
            .await
            .map_err(|err| TrackerError::LinearApiRequest(err.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| TrackerError::LinearUnknownPayload(err.to_string()))?;
        let payload: Value = serde_json::from_str(&text)
            .map_err(|err| TrackerError::LinearUnknownPayload(err.to_string()))?;
        if let Some(errors) = payload.get("errors") {
            return Err(TrackerError::LinearGraphqlErrors(errors.to_string()));
        }
        if !status.is_success() {
            return Err(TrackerError::LinearApiStatus(status.as_u16()));
        }
        Ok(payload)
    }

    async fn fetch_paginated(
        &self,
        query: &str,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        let mut all = Vec::new();
        let mut after: Option<String> = None;
        loop {
            let mut variables = json!({
                "projectSlug": self.config.project_slug,
                "activeStates": state_names,
                "stateNames": state_names,
                "first": self.config.page_size,
                "after": after,
            });
            if query == CANDIDATE_QUERY
                && let Some(map) = variables.as_object_mut()
            {
                map.insert(
                    "assigneeFilter".to_string(),
                    self.config.filter.assignee_filter_value(),
                );
                map.insert(
                    "labelFilter".to_string(),
                    self.config.filter.label_filter_value(),
                );
            }
            let payload = self.graphql(query, variables).await?;
            let issues = payload.pointer("/data/issues").ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing data.issues".to_string())
            })?;
            let nodes = issues
                .get("nodes")
                .and_then(Value::as_array)
                .ok_or_else(|| {
                    TrackerError::LinearUnknownPayload("missing issues.nodes".to_string())
                })?;
            all.extend(nodes.iter().filter_map(normalize_issue));
            let page = issues.get("pageInfo").ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing pageInfo".to_string())
            })?;
            let has_next = page
                .get("hasNextPage")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            if !has_next {
                return Ok(all);
            }
            let next = page
                .get("endCursor")
                .and_then(Value::as_str)
                .ok_or(TrackerError::LinearMissingEndCursor)?;
            after = Some(next.to_string());
        }
    }

    pub async fn attach_link_to_issue(
        &self,
        issue_id: &str,
        title: &str,
        url: &str,
    ) -> Result<(), TrackerError> {
        let issue = self.issue_link_target(issue_id, url).await?;
        if issue.linked {
            return Ok(());
        }
        let payload = self
            .graphql(
                ATTACHMENT_CREATE_MUTATION,
                json!({
                    "input": {
                        "issueId": issue.id,
                        "title": title,
                        "url": url,
                        "metadata": {
                            "source": "vik",
                            "kind": "github_pull_request",
                            "issueId": issue_id,
                        },
                    },
                }),
            )
            .await?;
        let success = payload
            .pointer("/data/attachmentCreate/success")
            .and_then(Value::as_bool)
            .unwrap_or(false);
        if !success {
            return Err(TrackerError::LinearUnknownPayload(
                "attachmentCreate.success was false".to_string(),
            ));
        }
        Ok(())
    }

    async fn issue_link_target(
        &self,
        issue_id: &str,
        url: &str,
    ) -> Result<IssueLinkTarget, TrackerError> {
        let payload = self
            .graphql(ISSUE_BY_ID_QUERY, json!({ "id": issue_id }))
            .await?;
        let issue = payload.pointer("/data/issue").ok_or_else(|| {
            TrackerError::LinearUnknownPayload(format!("missing data.issue for {issue_id}"))
        })?;
        let id = issue
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload(format!("missing data.issue.id for {issue_id}"))
            })?;
        Ok(IssueLinkTarget {
            id,
            linked: issue_has_attachment_url(issue, url),
        })
    }

    async fn issue_state_id(
        &self,
        issue_id: &str,
        state_name: &str,
    ) -> Result<String, TrackerError> {
        let payload = self
            .graphql(ISSUE_STATES_FOR_ISSUE_QUERY, json!({ "id": issue_id }))
            .await?;
        let states = payload
            .pointer("/data/issue/team/states/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing issue.team.states.nodes".to_string())
            })?;
        states
            .iter()
            .find(|state| {
                state
                    .get("name")
                    .and_then(Value::as_str)
                    .is_some_and(|name| name.eq_ignore_ascii_case(state_name))
            })
            .and_then(|state| state.get("id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload(format!(
                    "missing Linear state id for {state_name}"
                ))
            })
    }

    async fn issue_label_ids(
        &self,
        issue_id: &str,
        labels: &[String],
    ) -> Result<Vec<String>, TrackerError> {
        let payload = self
            .graphql(ISSUE_STATES_FOR_ISSUE_QUERY, json!({ "id": issue_id }))
            .await?;
        let issue = payload
            .pointer("/data/issue")
            .ok_or_else(|| TrackerError::LinearUnknownPayload("missing data.issue".to_string()))?;
        let mut ids: HashSet<String> = issue
            .pointer("/labels/nodes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|label| label.get("id").and_then(Value::as_str))
            .map(ToOwned::to_owned)
            .collect();
        let team_labels = issue
            .pointer("/team/labels/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing issue.team.labels.nodes".to_string())
            })?;
        for label in labels
            .iter()
            .map(|label| label.trim())
            .filter(|label| !label.is_empty())
        {
            let id = team_labels
                .iter()
                .find(|candidate| {
                    candidate
                        .get("name")
                        .and_then(Value::as_str)
                        .is_some_and(|name| name.eq_ignore_ascii_case(label))
                })
                .and_then(|candidate| candidate.get("id").and_then(Value::as_str))
                .ok_or_else(|| {
                    TrackerError::LinearUnknownPayload(format!(
                        "missing Linear label id for {label}"
                    ))
                })?;
            ids.insert(id.to_string());
        }
        let mut ids: Vec<_> = ids.into_iter().collect();
        ids.sort();
        Ok(ids)
    }

    async fn issue_by_id(&self, issue_id: &str) -> Result<Issue, TrackerError> {
        let payload = self
            .graphql(ISSUE_BY_ID_QUERY, json!({ "id": issue_id }))
            .await?;
        let issue = payload.pointer("/data/issue").ok_or_else(|| {
            TrackerError::LinearUnknownPayload(format!("missing data.issue for {issue_id}"))
        })?;
        normalize_issue(issue).ok_or_else(|| {
            TrackerError::LinearUnknownPayload(format!("could not normalize issue {issue_id}"))
        })
    }

    async fn comment_create(
        &self,
        issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        let payload = self
            .graphql(
                COMMENT_CREATE_MUTATION,
                json!({ "issueId": issue_id, "body": body }),
            )
            .await?;
        let comment = payload
            .pointer("/data/commentCreate/comment")
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing commentCreate.comment".to_string())
            })?;
        normalize_comment(comment)
    }

    async fn file_upload(
        &self,
        filename: &str,
        content_type: &str,
        bytes: Vec<u8>,
    ) -> Result<String, TrackerError> {
        let payload = self
            .graphql(
                FILE_UPLOAD_MUTATION,
                json!({
                    "filename": filename,
                    "contentType": content_type,
                    "size": bytes.len() as i64,
                    "makePublic": true,
                }),
            )
            .await?;
        let upload = payload
            .pointer("/data/fileUpload/uploadFile")
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing fileUpload.uploadFile".to_string())
            })?;
        let upload_url = upload
            .get("uploadUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| TrackerError::LinearUnknownPayload("missing uploadUrl".to_string()))?;
        let asset_url = upload
            .get("assetUrl")
            .and_then(Value::as_str)
            .ok_or_else(|| TrackerError::LinearUnknownPayload("missing assetUrl".to_string()))?;
        let headers = upload
            .get("headers")
            .and_then(Value::as_array)
            .map(|headers| linear_upload_headers(headers))
            .transpose()?
            .unwrap_or_default();
        let response = self
            .http
            .put(upload_url)
            .headers(headers)
            .body(bytes)
            .send()
            .await
            .map_err(|err| TrackerError::LinearApiRequest(err.to_string()))?;
        if !response.status().is_success() {
            return Err(TrackerError::LinearApiStatus(response.status().as_u16()));
        }
        Ok(asset_url.to_string())
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct IssueLinkTarget {
    id: String,
    linked: bool,
}

pub(crate) fn issue_has_attachment_url(issue: &Value, url: &str) -> bool {
    issue
        .pointer("/attachments/nodes")
        .and_then(Value::as_array)
        .is_some_and(|nodes| {
            nodes
                .iter()
                .any(|node| node.get("url").and_then(Value::as_str) == Some(url))
        })
}

#[async_trait]
impl Tracker for LinearClient {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_paginated(CANDIDATE_QUERY, &self.config.active_states)
            .await
    }

    async fn fetch_by_states(&self, state_names: &[String]) -> Result<Vec<Issue>, TrackerError> {
        if state_names.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch_paginated(ISSUES_BY_STATES_QUERY, state_names)
            .await
    }

    async fn fetch_states_by_ids(&self, issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        if issue_ids.is_empty() {
            return Ok(Vec::new());
        }
        let payload = self
            .graphql(ISSUE_STATES_BY_IDS_QUERY, json!({ "ids": issue_ids }))
            .await?;
        let nodes = payload
            .pointer("/data/issues/nodes")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing data.issues.nodes".to_string())
            })?;
        Ok(nodes.iter().filter_map(normalize_issue).collect())
    }

    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError> {
        self.issue_by_id(issue_id).await
    }

    async fn update_issue(
        &self,
        issue_id: &str,
        update: IssueUpdate,
    ) -> Result<Issue, TrackerError> {
        let mut input = Map::new();
        if let Some(state_name) = update
            .state
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            let state_id = self.issue_state_id(issue_id, state_name).await?;
            input.insert("stateId".to_string(), json!(state_id));
        }
        if !update.labels.is_empty() {
            input.insert(
                "labelIds".to_string(),
                json!(self.issue_label_ids(issue_id, &update.labels).await?),
            );
        }
        if input.is_empty() {
            return self.issue_by_id(issue_id).await;
        }
        let payload = self
            .graphql(
                ISSUE_UPDATE_MUTATION,
                json!({ "id": issue_id, "input": Value::Object(input) }),
            )
            .await?;
        let issue = payload.pointer("/data/issueUpdate/issue").ok_or_else(|| {
            TrackerError::LinearUnknownPayload("missing issueUpdate.issue".to_string())
        })?;
        normalize_issue(issue).ok_or_else(|| {
            TrackerError::LinearUnknownPayload(format!("could not normalize updated {issue_id}"))
        })
    }

    async fn create_comment(
        &self,
        issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        self.comment_create(issue_id, body).await
    }

    async fn update_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        let payload = self
            .graphql(
                COMMENT_UPDATE_MUTATION,
                json!({ "id": comment_id, "body": body }),
            )
            .await?;
        let comment = payload
            .pointer("/data/commentUpdate/comment")
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload("missing commentUpdate.comment".to_string())
            })?;
        normalize_comment(comment)
    }

    async fn upload_attachment(
        &self,
        issue_id: &str,
        path: &Path,
        content_type: &str,
    ) -> Result<IssueAttachment, TrackerError> {
        let filename = path
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("attachment");
        let bytes = std::fs::read(path)
            .map_err(|err| TrackerError::LinearApiRequest(format!("read attachment: {err}")))?;
        let asset_url = self.file_upload(filename, content_type, bytes).await?;
        let comment = self
            .comment_create(issue_id, &format!("[{filename}]({asset_url})"))
            .await?;
        Ok(IssueAttachment {
            url: asset_url,
            comment: Some(comment),
        })
    }

    async fn link_pr(&self, issue_id: &str, title: &str, url: &str) -> Result<(), TrackerError> {
        self.attach_link_to_issue(issue_id, title, url).await
    }
}

fn linear_upload_headers(headers: &[Value]) -> Result<HeaderMap, TrackerError> {
    let mut map = HeaderMap::new();
    for header in headers {
        let Some(key) = header.get("key").and_then(Value::as_str) else {
            continue;
        };
        let Some(value) = header.get("value").and_then(Value::as_str) else {
            continue;
        };
        let name = HeaderName::from_bytes(key.as_bytes())
            .map_err(|err| TrackerError::LinearUnknownPayload(err.to_string()))?;
        let value = HeaderValue::from_str(value)
            .map_err(|err| TrackerError::LinearUnknownPayload(err.to_string()))?;
        map.insert(name, value);
    }
    Ok(map)
}

fn normalize_comment(comment: &Value) -> Result<IssueComment, TrackerError> {
    let id = comment
        .get("id")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
        .ok_or_else(|| TrackerError::LinearUnknownPayload("missing comment.id".to_string()))?;
    let body = comment
        .get("body")
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    let url = comment
        .get("url")
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);
    Ok(IssueComment { id, body, url })
}
