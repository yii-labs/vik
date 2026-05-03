use async_trait::async_trait;
use serde_json::{Value, json};
use vik_core::{Issue, IssueTracker, TrackerError};

use crate::normalize::normalize_issue;
use crate::queries::{
    ATTACHMENT_CREATE_MUTATION, CANDIDATE_QUERY, ISSUE_BY_IDENTIFIER_QUERY,
    ISSUE_STATES_BY_IDS_QUERY, ISSUES_BY_STATES_QUERY,
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
    pub assignee: Vec<String>,
    pub tag: Vec<String>,
}

impl LinearIssueFilterConfig {
    pub fn new(assignee: Vec<String>, tag: Vec<String>) -> Self {
        Self {
            assignee: clean_filter_values(assignee),
            tag: clean_filter_values(tag),
        }
    }

    pub(crate) fn assignee_filter_value(&self) -> Value {
        let clauses = self
            .assignee
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
            .tag
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
        issue_identifier: &str,
        title: &str,
        url: &str,
    ) -> Result<(), TrackerError> {
        let issue = self.issue_link_target(issue_identifier, url).await?;
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
                            "issueIdentifier": issue_identifier,
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
        issue_identifier: &str,
        url: &str,
    ) -> Result<IssueLinkTarget, TrackerError> {
        let payload = self
            .graphql(ISSUE_BY_IDENTIFIER_QUERY, json!({ "id": issue_identifier }))
            .await?;
        let issue = payload.pointer("/data/issue").ok_or_else(|| {
            TrackerError::LinearUnknownPayload(format!("missing data.issue for {issue_identifier}"))
        })?;
        let id = issue
            .get("id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .ok_or_else(|| {
                TrackerError::LinearUnknownPayload(format!(
                    "missing data.issue.id for {issue_identifier}"
                ))
            })?;
        Ok(IssueLinkTarget {
            id,
            linked: issue_has_attachment_url(issue, url),
        })
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
impl IssueTracker for LinearClient {
    async fn fetch_candidate_issues(&self) -> Result<Vec<Issue>, TrackerError> {
        self.fetch_paginated(CANDIDATE_QUERY, &self.config.active_states)
            .await
    }

    async fn fetch_issues_by_states(
        &self,
        state_names: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
        if state_names.is_empty() {
            return Ok(Vec::new());
        }
        self.fetch_paginated(ISSUES_BY_STATES_QUERY, state_names)
            .await
    }

    async fn fetch_issue_states_by_ids(
        &self,
        issue_ids: &[String],
    ) -> Result<Vec<Issue>, TrackerError> {
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
}
