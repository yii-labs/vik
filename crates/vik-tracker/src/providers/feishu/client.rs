use std::collections::HashSet;
use std::path::Path;
use std::process::Command;

use async_trait::async_trait;
use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use vik_core::{
    Issue, IssueAttachment, IssueComment, IssueTracker, IssueUpdate, TrackerError, normalize_state,
};

pub const DEFAULT_PAGE_SIZE: usize = 100;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FeishuIssueFields {
    pub identifier: String,
    pub title: String,
    pub description: String,
    pub state: String,
    pub delegated: String,
    pub labels: String,
    pub comments: String,
    pub pr_links: String,
}

impl FeishuIssueFields {
    pub fn names(&self) -> Vec<String> {
        let mut seen = HashSet::new();
        [
            &self.title,
            &self.identifier,
            &self.state,
            &self.description,
            &self.labels,
            &self.comments,
            &self.pr_links,
            &self.delegated,
        ]
        .into_iter()
        .map(|field| field.trim())
        .filter(|field| !field.is_empty())
        .filter_map(|field| {
            if seen.insert(field.to_string()) {
                Some(field.to_string())
            } else {
                None
            }
        })
        .collect()
    }
}

#[derive(Debug, Clone)]
pub struct FeishuClientConfig {
    pub cli_path: String,
    pub base_token: String,
    pub table_id: String,
    pub view_id: String,
    pub identity: String,
    pub active_states: Vec<String>,
    pub filter_tags: Vec<String>,
    pub fields: FeishuIssueFields,
    pub page_size: usize,
}

impl FeishuClientConfig {
    pub fn new(
        cli_path: impl Into<String>,
        base_token: impl Into<String>,
        table_id: impl Into<String>,
        identity: impl Into<String>,
        active_states: Vec<String>,
        fields: FeishuIssueFields,
    ) -> Self {
        Self {
            cli_path: cli_path.into(),
            base_token: base_token.into(),
            table_id: table_id.into(),
            view_id: String::new(),
            identity: identity.into(),
            active_states,
            filter_tags: Vec::new(),
            fields,
            page_size: DEFAULT_PAGE_SIZE,
        }
    }

    pub fn with_view_id(mut self, view_id: impl Into<String>) -> Self {
        self.view_id = view_id.into().trim().to_string();
        self
    }

    pub fn with_filter_tags(mut self, tags: Vec<String>) -> Self {
        self.filter_tags = clean_values(tags);
        self
    }
}

#[derive(Debug, Clone)]
pub struct FeishuClient {
    config: FeishuClientConfig,
}

#[derive(Debug, Clone)]
pub(crate) struct FeishuRecord {
    pub id: String,
    pub fields: Map<String, Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredComment {
    id: String,
    body: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredPrLink {
    title: String,
    url: String,
}

impl FeishuClient {
    pub fn new(config: FeishuClientConfig) -> Result<Self, TrackerError> {
        if config.cli_path.trim().is_empty() {
            return Err(TrackerError::MissingTrackerCliPath);
        }
        if config.base_token.trim().is_empty() {
            return Err(TrackerError::MissingTrackerBaseToken);
        }
        if config.table_id.trim().is_empty() {
            return Err(TrackerError::MissingTrackerTableId);
        }
        if !matches!(config.identity.as_str(), "user" | "bot") {
            return Err(TrackerError::InvalidTrackerCliIdentity(config.identity));
        }
        Ok(Self { config })
    }

    async fn run_cli_json(&self, args: Vec<String>) -> Result<Value, TrackerError> {
        let cli_path = self.config.cli_path.clone();
        tokio::task::spawn_blocking(move || run_command_json(cli_path, args))
            .await
            .map_err(|err| TrackerError::FeishuCli(err.to_string()))?
    }

    async fn list_records(&self) -> Result<Vec<FeishuRecord>, TrackerError> {
        let mut all = Vec::new();
        let mut offset = 0usize;
        loop {
            let mut args = self.base_args("+record-list");
            for field in self.config.fields.names() {
                args.push("--field-id".to_string());
                args.push(field);
            }
            if !self.config.view_id.is_empty() {
                args.extend(["--view-id".to_string(), self.config.view_id.clone()]);
            }
            args.extend([
                "--limit".to_string(),
                self.config.page_size.to_string(),
                "--offset".to_string(),
                offset.to_string(),
                "--format".to_string(),
                "json".to_string(),
            ]);
            let payload = self.run_cli_json(args).await?;
            let has_more = payload
                .pointer("/data/has_more")
                .and_then(Value::as_bool)
                .unwrap_or(false);
            all.extend(records_from_list_payload(&payload)?);
            if !has_more {
                return Ok(all);
            }
            offset = offset.saturating_add(self.config.page_size);
        }
    }

    async fn record_by_id(&self, record_id: &str) -> Result<FeishuRecord, TrackerError> {
        let mut args = self.base_args("+record-get");
        // +record-get has no --format flag; it returns a JSON envelope by default.
        args.extend(["--record-id".to_string(), record_id.to_string()]);
        let payload = self.run_cli_json(args).await?;
        let fields = record_fields_from_get_payload(&payload)?;
        Ok(FeishuRecord {
            id: record_id.to_string(),
            fields,
        })
    }

    async fn search_by_identifier(&self, identifier: &str) -> Result<FeishuRecord, TrackerError> {
        let search = json!({
            "keyword": identifier,
            "search_fields": [self.config.fields.identifier],
            "select_fields": self.config.fields.names(),
            "limit": self.config.page_size,
        });
        let mut args = self.base_args("+record-search");
        args.extend([
            "--json".to_string(),
            search.to_string(),
            "--format".to_string(),
            "json".to_string(),
        ]);
        let payload = self.run_cli_json(args).await?;
        records_from_list_payload(&payload)?
            .into_iter()
            .find(|record| {
                field_text(&record.fields, &self.config.fields.identifier)
                    .is_some_and(|value| value == identifier)
            })
            .ok_or_else(|| {
                TrackerError::FeishuUnknownPayload(format!(
                    "missing exact record for identifier {identifier}"
                ))
            })
    }

    async fn record_for_issue(&self, issue_id: &str) -> Result<FeishuRecord, TrackerError> {
        if issue_id.starts_with("rec") || self.config.fields.identifier.trim().is_empty() {
            return self.record_by_id(issue_id).await;
        }
        self.search_by_identifier(issue_id).await
    }

    async fn patch_record(
        &self,
        record_id: &str,
        fields: Map<String, Value>,
    ) -> Result<FeishuRecord, TrackerError> {
        if fields.is_empty() {
            return self.record_by_id(record_id).await;
        }
        let mut args = self.base_args("+record-upsert");
        // +record-upsert has no --format flag; it returns a JSON envelope by default.
        args.extend([
            "--record-id".to_string(),
            record_id.to_string(),
            "--json".to_string(),
            Value::Object(fields).to_string(),
        ]);
        self.run_cli_json(args).await?;
        self.record_by_id(record_id).await
    }

    fn base_args(&self, command: &str) -> Vec<String> {
        vec![
            "base".to_string(),
            command.to_string(),
            "--base-token".to_string(),
            self.config.base_token.clone(),
            "--table-id".to_string(),
            self.config.table_id.clone(),
            "--as".to_string(),
            self.config.identity.clone(),
        ]
    }

    fn normalize_issue(&self, record: &FeishuRecord) -> Issue {
        let identifier = field_text(&record.fields, &self.config.fields.identifier)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| record.id.clone());
        let title = field_text(&record.fields, &self.config.fields.title)
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| identifier.clone());
        let state = field_text(&record.fields, &self.config.fields.state).unwrap_or_default();
        let labels = parse_labels(field_text(&record.fields, &self.config.fields.labels));
        Issue {
            id: record.id.clone(),
            identifier,
            title,
            description: field_text(&record.fields, &self.config.fields.description),
            priority: None,
            state,
            branch_name: None,
            url: Some(format!(
                "https://www.feishu.cn/base/{}?table={}",
                self.config.base_token, self.config.table_id
            )),
            labels,
            blocked_by: Vec::new(),
            created_at: None,
            updated_at: None,
        }
    }

    fn is_candidate(&self, record: &FeishuRecord, issue: &Issue) -> bool {
        if !self.config.fields.delegated.trim().is_empty()
            && field_bool(&record.fields, &self.config.fields.delegated) != Some(true)
        {
            return false;
        }
        if self.config.filter_tags.is_empty() {
            return true;
        }
        let issue_labels: HashSet<_> = issue
            .labels
            .iter()
            .map(|label| normalize_state(label))
            .collect();
        self.config
            .filter_tags
            .iter()
            .any(|tag| issue_labels.contains(&normalize_state(tag)))
    }

    fn comments(&self, record: &FeishuRecord) -> Result<Vec<StoredComment>, TrackerError> {
        parse_json_field(field_text(&record.fields, &self.config.fields.comments).as_deref())
    }

    fn pr_links(&self, record: &FeishuRecord) -> Result<Vec<StoredPrLink>, TrackerError> {
        parse_json_field(field_text(&record.fields, &self.config.fields.pr_links).as_deref())
    }
}

#[async_trait]
impl IssueTracker for FeishuClient {
    async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
        if self.config.active_states.is_empty() {
            return Ok(Vec::new());
        }
        let active_states = self.config.active_states.clone();
        let records = self.list_records().await?;
        Ok(records
            .into_iter()
            .filter_map(|record| {
                let issue = self.normalize_issue(&record);
                issue_matches_state(&issue, &active_states)
                    .then(|| self.is_candidate(&record, &issue).then_some(issue))
                    .flatten()
            })
            .collect())
    }

    async fn fetch_by_states(&self, state_names: &[String]) -> Result<Vec<Issue>, TrackerError> {
        if state_names.is_empty() {
            return Ok(Vec::new());
        }
        let records = self.list_records().await?;
        Ok(records
            .into_iter()
            .map(|record| self.normalize_issue(&record))
            .filter(|issue| issue_matches_state(issue, state_names))
            .collect())
    }

    async fn fetch_states_by_ids(&self, issue_ids: &[String]) -> Result<Vec<Issue>, TrackerError> {
        let mut issues = Vec::new();
        for issue_id in issue_ids {
            issues.push(self.get_issue(issue_id).await?);
        }
        Ok(issues)
    }

    async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError> {
        let record = self.record_for_issue(issue_id).await?;
        Ok(self.normalize_issue(&record))
    }

    async fn update_issue(
        &self,
        issue_id: &str,
        update: IssueUpdate,
    ) -> Result<Issue, TrackerError> {
        let record = self.record_for_issue(issue_id).await?;
        let mut patch = Map::new();
        if let Some(state) = update
            .state
            .as_deref()
            .map(str::trim)
            .filter(|state| !state.is_empty())
        {
            patch.insert(self.config.fields.state.clone(), json!(state));
        }
        if !update.labels.is_empty() {
            let mut labels = parse_labels(field_text(&record.fields, &self.config.fields.labels));
            for label in clean_values(update.labels) {
                if !labels.iter().any(|existing| same_text(existing, &label)) {
                    labels.push(label);
                }
            }
            patch.insert(self.config.fields.labels.clone(), json!(labels.join(", ")));
        }
        let updated = self.patch_record(&record.id, patch).await?;
        Ok(self.normalize_issue(&updated))
    }

    async fn create_comment(
        &self,
        issue_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        let record = self.record_for_issue(issue_id).await?;
        let mut comments = self.comments(&record)?;
        let comment = StoredComment {
            id: format!(
                "{}:comment:{}-{}",
                record.id,
                Utc::now().timestamp_millis(),
                comments.len() + 1
            ),
            body: body.to_string(),
            url: None,
        };
        comments.push(comment.clone());
        let mut patch = Map::new();
        patch.insert(
            self.config.fields.comments.clone(),
            json!(
                serde_json::to_string(&comments)
                    .map_err(|err| { TrackerError::FeishuUnknownPayload(err.to_string()) })?
            ),
        );
        self.patch_record(&record.id, patch).await?;
        Ok(comment.into())
    }

    async fn list_comments(&self, issue_id: &str) -> Result<Vec<IssueComment>, TrackerError> {
        let record = self.record_for_issue(issue_id).await?;
        Ok(self
            .comments(&record)?
            .into_iter()
            .map(IssueComment::from)
            .collect())
    }

    async fn update_comment(
        &self,
        comment_id: &str,
        body: &str,
    ) -> Result<IssueComment, TrackerError> {
        let record_id = comment_id
            .split_once(":comment:")
            .map(|(record_id, _)| record_id.to_string())
            .ok_or_else(|| {
                TrackerError::FeishuUnknownPayload(format!("invalid comment id {comment_id}"))
            })?;
        let record = self.record_by_id(&record_id).await?;
        let mut comments = self.comments(&record)?;
        let mut updated = None;
        for comment in &mut comments {
            if comment.id == comment_id {
                comment.body = body.to_string();
                updated = Some(comment.clone());
                break;
            }
        }
        let updated = updated.ok_or_else(|| {
            TrackerError::FeishuUnknownPayload(format!("missing comment {comment_id}"))
        })?;
        let mut patch = Map::new();
        patch.insert(
            self.config.fields.comments.clone(),
            json!(
                serde_json::to_string(&comments)
                    .map_err(|err| { TrackerError::FeishuUnknownPayload(err.to_string()) })?
            ),
        );
        self.patch_record(&record.id, patch).await?;
        Ok(updated.into())
    }

    async fn upload_attachment(
        &self,
        _issue_id: &str,
        _path: &Path,
        _content_type: &str,
    ) -> Result<IssueAttachment, TrackerError> {
        Err(TrackerError::UnsupportedTrackerOperation(
            "feishu attachment upload requires a configured Base attachment field".to_string(),
        ))
    }

    async fn link_pr(&self, issue_id: &str, title: &str, url: &str) -> Result<(), TrackerError> {
        let record = self.record_for_issue(issue_id).await?;
        let mut links = self.pr_links(&record)?;
        if links.iter().any(|link| link.url == url) {
            return Ok(());
        }
        links.push(StoredPrLink {
            title: title.to_string(),
            url: url.to_string(),
        });
        let mut patch = Map::new();
        patch.insert(
            self.config.fields.pr_links.clone(),
            json!(
                serde_json::to_string(&links)
                    .map_err(|err| { TrackerError::FeishuUnknownPayload(err.to_string()) })?
            ),
        );
        self.patch_record(&record.id, patch).await?;
        Ok(())
    }
}

impl From<StoredComment> for IssueComment {
    fn from(value: StoredComment) -> Self {
        Self {
            id: value.id,
            body: value.body,
            url: value.url,
        }
    }
}

fn run_command_json(cli_path: String, args: Vec<String>) -> Result<Value, TrackerError> {
    let output = Command::new(&cli_path)
        .args(&args)
        .output()
        .map_err(|err| TrackerError::FeishuCli(format!("{cli_path}: {err}")))?;
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    if !output.status.success() {
        let detail = [stdout.trim(), stderr.trim()]
            .into_iter()
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>()
            .join("\n");
        return Err(TrackerError::FeishuCli(detail));
    }
    let payload: Value = serde_json::from_str(&stdout)
        .map_err(|err| TrackerError::FeishuUnknownPayload(err.to_string()))?;
    if payload.get("ok").and_then(Value::as_bool) == Some(false) {
        return Err(TrackerError::FeishuCli(
            payload.get("error").unwrap_or(&payload).to_string(),
        ));
    }
    Ok(payload)
}

pub(crate) fn records_from_list_payload(
    payload: &Value,
) -> Result<Vec<FeishuRecord>, TrackerError> {
    let fields = payload
        .pointer("/data/fields")
        .and_then(Value::as_array)
        .ok_or_else(|| TrackerError::FeishuUnknownPayload("missing data.fields".to_string()))?;
    let record_ids = payload
        .pointer("/data/record_id_list")
        .and_then(Value::as_array)
        .ok_or_else(|| {
            TrackerError::FeishuUnknownPayload("missing data.record_id_list".to_string())
        })?;
    let rows = payload
        .pointer("/data/data")
        .and_then(Value::as_array)
        .ok_or_else(|| TrackerError::FeishuUnknownPayload("missing data.data".to_string()))?;
    if record_ids.len() != rows.len() {
        return Err(TrackerError::FeishuUnknownPayload(
            "record_id_list and data length mismatch".to_string(),
        ));
    }

    let mut records = Vec::new();
    for (index, row) in rows.iter().enumerate() {
        let values = row.as_array().ok_or_else(|| {
            TrackerError::FeishuUnknownPayload("record data row was not an array".to_string())
        })?;
        let id = record_ids
            .get(index)
            .and_then(Value::as_str)
            .ok_or_else(|| {
                TrackerError::FeishuUnknownPayload("record id was not a string".to_string())
            })?
            .to_string();
        let mut map = Map::new();
        for (field_index, field) in fields.iter().enumerate() {
            let name = field.as_str().ok_or_else(|| {
                TrackerError::FeishuUnknownPayload("field name was not a string".to_string())
            })?;
            let value = values.get(field_index).cloned().unwrap_or(Value::Null);
            map.insert(name.to_string(), value);
        }
        records.push(FeishuRecord { id, fields: map });
    }
    Ok(records)
}

pub(crate) fn record_fields_from_get_payload(
    payload: &Value,
) -> Result<Map<String, Value>, TrackerError> {
    let record = payload
        .pointer("/data/record")
        .and_then(Value::as_object)
        .ok_or_else(|| TrackerError::FeishuUnknownPayload("missing data.record".to_string()))?;
    Ok(record
        .get("fields")
        .and_then(Value::as_object)
        .unwrap_or(record)
        .clone())
}

#[cfg(test)]
pub(crate) fn issue_from_record(record: &FeishuRecord, fields: &FeishuIssueFields) -> Issue {
    let config = FeishuClientConfig::new("", "base", "table", "user", vec![], fields.clone());
    FeishuClient { config }.normalize_issue(record)
}

fn field_text(fields: &Map<String, Value>, field: &str) -> Option<String> {
    fields
        .get(field)
        .and_then(cell_text)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn field_bool(fields: &Map<String, Value>, field: &str) -> Option<bool> {
    match fields.get(field)? {
        Value::Bool(value) => Some(*value),
        Value::String(value) => match normalize_state(value).as_str() {
            "true" | "yes" | "1" => Some(true),
            "false" | "no" | "0" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

fn cell_text(value: &Value) -> Option<String> {
    match value {
        Value::Null => None,
        Value::String(value) => Some(value.clone()),
        Value::Number(value) => Some(value.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        Value::Array(values) => {
            let parts: Vec<_> = values.iter().filter_map(cell_text).collect();
            (!parts.is_empty()).then(|| parts.join(", "))
        }
        Value::Object(map) => ["text", "name", "url", "link"]
            .into_iter()
            .find_map(|key| map.get(key).and_then(Value::as_str).map(str::to_string)),
    }
}

fn parse_labels(value: Option<String>) -> Vec<String> {
    let Some(value) = value else {
        return Vec::new();
    };
    if let Ok(labels) = serde_json::from_str::<Vec<String>>(&value) {
        return clean_values(labels);
    }
    clean_values(value.split([',', '\n']).map(str::to_string).collect())
}

fn parse_json_field<T>(value: Option<&str>) -> Result<Vec<T>, TrackerError>
where
    T: for<'de> Deserialize<'de>,
{
    let Some(value) = value.map(str::trim).filter(|value| !value.is_empty()) else {
        return Ok(Vec::new());
    };
    serde_json::from_str(value).map_err(|err| {
        TrackerError::FeishuUnknownPayload(format!("could not parse stored JSON field: {err}"))
    })
}

fn clean_values(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .collect()
}

fn same_text(left: &str, right: &str) -> bool {
    normalize_state(left) == normalize_state(right)
}

fn issue_matches_state(issue: &Issue, states: &[String]) -> bool {
    states
        .iter()
        .any(|state| normalize_state(state) == issue.normalized_state())
}
