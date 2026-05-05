use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde::Serialize;
use serde_json::{Value, json};
use vik_core::{IssueTracker, IssueUpdate, TrackerError};

const VIK_ISSUE_TOOL: &str = "vik_issue";
const GET_ISSUE_ACTION: &str = "get_issue";
const LIST_COMMENTS_ACTION: &str = "list_comments";
const UPDATE_ISSUE_ACTION: &str = "update_issue";
const CREATE_COMMENT_ACTION: &str = "create_comment";
const UPDATE_COMMENT_ACTION: &str = "update_comment";
const UPLOAD_ATTACHMENT_ACTION: &str = "upload_attachment";
const LINK_PR_ACTION: &str = "link_pr";

#[derive(Clone, Default)]
pub(crate) struct DynamicTools {
    tracker: Option<Arc<dyn IssueTracker>>,
    workspace_root: Option<PathBuf>,
}

impl fmt::Debug for DynamicTools {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DynamicTools")
            .field("tracker", &self.tracker.is_some())
            .field("workspace_root", &self.workspace_root)
            .finish()
    }
}

impl DynamicTools {
    pub(crate) fn from_tracker(tracker: Arc<dyn IssueTracker>) -> Self {
        Self {
            tracker: Some(tracker),
            workspace_root: None,
        }
    }

    pub(crate) fn with_workspace_root(mut self, workspace_root: impl Into<PathBuf>) -> Self {
        self.workspace_root = Some(workspace_root.into());
        self
    }

    pub(crate) fn definitions(&self) -> Vec<Value> {
        if self.tracker.is_none() {
            return Vec::new();
        }
        vec![tracker_definition(
            VIK_ISSUE_TOOL,
            "Run a common issue operation against Vik's configured tracker.",
            json!({
                "action": {
                    "type": "string",
                    "description": "Tracker operation to run.",
                    "enum": [
                        GET_ISSUE_ACTION,
                        LIST_COMMENTS_ACTION,
                        UPDATE_ISSUE_ACTION,
                        CREATE_COMMENT_ACTION,
                        UPDATE_COMMENT_ACTION,
                        UPLOAD_ATTACHMENT_ACTION,
                        LINK_PR_ACTION
                    ]
                },
                "issue_id": string_schema("Provider-specific issue id."),
                "comment_id": string_schema("Provider-specific comment id."),
                "state": string_schema("Optional tracker state to set."),
                "labels": {
                    "type": "array",
                    "description": "Optional labels to add or set through the configured tracker.",
                    "items": { "type": "string" }
                },
                "body": string_schema("Comment body."),
                "path": string_schema("Path to a file inside the issue workspace."),
                "content_type": string_schema("Attachment content type."),
                "title": string_schema("Pull request title."),
                "url": string_schema("Pull request URL.")
            }),
            vec!["action"],
        )]
    }

    pub(crate) async fn handle_call(&self, params: &Value) -> Value {
        let Some(tool) = extract_tool_name(params) else {
            return tool_failure("missing dynamic tool name");
        };
        if tool != VIK_ISSUE_TOOL {
            return tool_failure(format!("unsupported dynamic tool call: {tool}"));
        }
        let Some(tracker) = &self.tracker else {
            return tool_failure("tracker dynamic tools are not configured");
        };
        let arguments = match extract_tool_arguments(params) {
            Ok(arguments) => arguments,
            Err(err) => return tool_failure(err),
        };
        let action = match required_string(&arguments, "action") {
            Ok(action) => action,
            Err(err) => return tool_failure(err),
        };
        match action.as_str() {
            GET_ISSUE_ACTION => {
                let issue_id = match required_string(&arguments, "issue_id") {
                    Ok(issue_id) => issue_id,
                    Err(err) => return tool_failure(err),
                };
                tool_result(tracker.get_issue(&issue_id).await)
            }
            LIST_COMMENTS_ACTION => {
                let issue_id = match required_string(&arguments, "issue_id") {
                    Ok(issue_id) => issue_id,
                    Err(err) => return tool_failure(err),
                };
                tool_result(tracker.list_comments(&issue_id).await)
            }
            UPDATE_ISSUE_ACTION => {
                let issue_id = match required_string(&arguments, "issue_id") {
                    Ok(issue_id) => issue_id,
                    Err(err) => return tool_failure(err),
                };
                let update = match issue_update_from_arguments(&arguments) {
                    Ok(update) => update,
                    Err(err) => return tool_failure(err),
                };
                tool_result(tracker.update_issue(&issue_id, update).await)
            }
            CREATE_COMMENT_ACTION => {
                let issue_id = match required_string(&arguments, "issue_id") {
                    Ok(issue_id) => issue_id,
                    Err(err) => return tool_failure(err),
                };
                let body = match required_string(&arguments, "body") {
                    Ok(body) => body,
                    Err(err) => return tool_failure(err),
                };
                tool_result(tracker.create_comment(&issue_id, &body).await)
            }
            UPDATE_COMMENT_ACTION => {
                let comment_id = match required_string(&arguments, "comment_id") {
                    Ok(comment_id) => comment_id,
                    Err(err) => return tool_failure(err),
                };
                let body = match required_string(&arguments, "body") {
                    Ok(body) => body,
                    Err(err) => return tool_failure(err),
                };
                tool_result(tracker.update_comment(&comment_id, &body).await)
            }
            UPLOAD_ATTACHMENT_ACTION => {
                let issue_id = match required_string(&arguments, "issue_id") {
                    Ok(issue_id) => issue_id,
                    Err(err) => return tool_failure(err),
                };
                let raw_path = match required_string(&arguments, "path") {
                    Ok(path) => path,
                    Err(err) => return tool_failure(err),
                };
                let content_type = match required_string(&arguments, "content_type") {
                    Ok(content_type) => content_type,
                    Err(err) => return tool_failure(err),
                };
                let path = match self.workspace_file_path(&raw_path) {
                    Ok(path) => path,
                    Err(err) => return tool_failure(err),
                };
                tool_result(
                    tracker
                        .upload_attachment(&issue_id, &path, &content_type)
                        .await,
                )
            }
            LINK_PR_ACTION => {
                let issue_id = match required_string(&arguments, "issue_id") {
                    Ok(issue_id) => issue_id,
                    Err(err) => return tool_failure(err),
                };
                let title = match required_string(&arguments, "title") {
                    Ok(title) => title,
                    Err(err) => return tool_failure(err),
                };
                let url = match required_string(&arguments, "url") {
                    Ok(url) => url,
                    Err(err) => return tool_failure(err),
                };
                match tracker.link_pr(&issue_id, &title, &url).await {
                    Ok(()) => tool_success(compact_json(&json!({ "linked": true }))),
                    Err(err) => tool_failure(err.to_string()),
                }
            }
            _ => tool_failure(format!("unsupported vik_issue action: {action}")),
        }
    }

    fn workspace_file_path(&self, raw_path: &str) -> Result<PathBuf, String> {
        let Some(workspace_root) = &self.workspace_root else {
            return Err("workspace root is not configured for attachment upload".to_string());
        };
        let root = workspace_root
            .canonicalize()
            .map_err(|err| format!("workspace root could not be resolved: {err}"))?;
        let raw = Path::new(raw_path);
        let candidate = if raw.is_absolute() {
            raw.to_path_buf()
        } else {
            root.join(raw)
        };
        let path = candidate
            .canonicalize()
            .map_err(|err| format!("attachment path could not be resolved: {err}"))?;
        if !path.starts_with(&root) {
            return Err("attachment path must stay inside the issue workspace".to_string());
        }
        if !path.is_file() {
            return Err("attachment path must point to a file".to_string());
        }
        Ok(path)
    }
}

fn tracker_definition(
    name: &str,
    description: &str,
    properties: Value,
    required: Vec<&str>,
) -> Value {
    json!({
        "name": name,
        "description": description,
        "deferLoading": false,
        "inputSchema": {
            "type": "object",
            "properties": properties,
            "required": required,
            "additionalProperties": false
        }
    })
}

fn string_schema(description: &str) -> Value {
    json!({
        "type": "string",
        "description": description
    })
}

fn extract_tool_name(params: &Value) -> Option<String> {
    [
        "/tool",
        "/name",
        "/toolName",
        "/tool_name",
        "/tool/name",
        "/toolCall/name",
        "/function/name",
    ]
    .into_iter()
    .find_map(|path| {
        params
            .pointer(path)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|name| !name.is_empty())
            .map(ToOwned::to_owned)
    })
}

fn extract_tool_arguments(params: &Value) -> Result<Value, String> {
    let Some(arguments) = [
        "/arguments",
        "/input",
        "/args",
        "/toolInput",
        "/tool/arguments",
        "/toolCall/arguments",
        "/function/arguments",
    ]
    .into_iter()
    .find_map(|path| params.pointer(path)) else {
        return Ok(json!({}));
    };
    match arguments {
        Value::Object(_) => Ok(arguments.clone()),
        Value::String(raw) => serde_json::from_str(raw)
            .map_err(|err| format!("dynamic tool arguments are not valid JSON: {err}")),
        Value::Null => Ok(json!({})),
        _ => Err("dynamic tool arguments must be an object".to_string()),
    }
}

fn issue_update_from_arguments(arguments: &Value) -> Result<IssueUpdate, String> {
    let state = optional_string(arguments, "state")?;
    let labels = optional_string_array(arguments, "labels")?;
    Ok(IssueUpdate { state, labels })
}

fn required_string(arguments: &Value, field: &str) -> Result<String, String> {
    optional_string(arguments, field)?.ok_or_else(|| format!("{field} is required"))
}

fn optional_string(arguments: &Value, field: &str) -> Result<Option<String>, String> {
    match arguments.get(field) {
        Some(Value::String(value)) => {
            Ok(Some(value.trim().to_string()).filter(|value| !value.is_empty()))
        }
        Some(Value::Null) | None => Ok(None),
        Some(_) => Err(format!("{field} must be a string")),
    }
}

fn optional_string_array(arguments: &Value, field: &str) -> Result<Vec<String>, String> {
    let Some(value) = arguments.get(field) else {
        return Ok(Vec::new());
    };
    let Value::Array(items) = value else {
        return Err(format!("{field} must be an array of strings"));
    };
    items
        .iter()
        .map(|item| {
            item.as_str()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .ok_or_else(|| format!("{field} must be an array of strings"))
        })
        .collect()
}

fn tool_result<T>(result: Result<T, TrackerError>) -> Value
where
    T: Serialize,
{
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(value) => tool_success(compact_json(&value)),
            Err(err) => tool_failure(format!("dynamic tool response serialization failed: {err}")),
        },
        Err(err) => tool_failure(err.to_string()),
    }
}

fn tool_success(text: String) -> Value {
    json!({
        "success": true,
        "contentItems": [
            { "type": "inputText", "text": text }
        ]
    })
}

fn tool_failure(message: impl Into<String>) -> Value {
    json!({
        "success": false,
        "contentItems": [
            { "type": "inputText", "text": message.into() }
        ]
    })
}

fn compact_json(value: &Value) -> String {
    serde_json::to_string(value).unwrap_or_else(|_| value.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::{TimeZone, Utc};
    use tempfile::tempdir;
    use vik_core::{Issue, IssueAttachment, IssueComment};

    #[derive(Debug)]
    struct TestTracker;

    #[async_trait::async_trait]
    impl IssueTracker for TestTracker {
        async fn fetch_candidates(&self) -> Result<Vec<Issue>, TrackerError> {
            Ok(vec![])
        }

        async fn fetch_by_states(
            &self,
            _state_names: &[String],
        ) -> Result<Vec<Issue>, TrackerError> {
            Ok(vec![])
        }

        async fn fetch_states_by_ids(
            &self,
            _issue_ids: &[String],
        ) -> Result<Vec<Issue>, TrackerError> {
            Ok(vec![])
        }

        async fn get_issue(&self, issue_id: &str) -> Result<Issue, TrackerError> {
            Ok(issue(issue_id, "Todo"))
        }

        async fn update_issue(
            &self,
            issue_id: &str,
            update: IssueUpdate,
        ) -> Result<Issue, TrackerError> {
            Ok(issue(issue_id, update.state.as_deref().unwrap_or("Todo")))
        }

        async fn create_comment(
            &self,
            _issue_id: &str,
            body: &str,
        ) -> Result<IssueComment, TrackerError> {
            Ok(IssueComment {
                id: "comment-1".to_string(),
                body: body.to_string(),
                url: None,
            })
        }

        async fn list_comments(&self, issue_id: &str) -> Result<Vec<IssueComment>, TrackerError> {
            Ok(vec![IssueComment {
                id: "comment-1".to_string(),
                body: format!("workpad for {issue_id}"),
                url: None,
            }])
        }

        async fn update_comment(
            &self,
            comment_id: &str,
            body: &str,
        ) -> Result<IssueComment, TrackerError> {
            Ok(IssueComment {
                id: comment_id.to_string(),
                body: body.to_string(),
                url: None,
            })
        }

        async fn upload_attachment(
            &self,
            _issue_id: &str,
            path: &Path,
            _content_type: &str,
        ) -> Result<IssueAttachment, TrackerError> {
            Ok(IssueAttachment {
                url: path.display().to_string(),
                comment: None,
            })
        }

        async fn link_pr(
            &self,
            _issue_id: &str,
            _title: &str,
            _url: &str,
        ) -> Result<(), TrackerError> {
            Ok(())
        }
    }

    fn tools() -> DynamicTools {
        let tracker: Arc<dyn IssueTracker> = Arc::new(TestTracker);
        DynamicTools::from_tracker(tracker)
    }

    fn issue(id: &str, state: &str) -> Issue {
        Issue {
            id: id.to_string(),
            identifier: format!("ISSUE-{id}"),
            title: "Title".to_string(),
            description: None,
            priority: None,
            state: state.to_string(),
            branch_name: None,
            url: None,
            labels: vec![],
            blocked_by: vec![],
            created_at: Some(Utc.with_ymd_and_hms(2026, 5, 5, 0, 0, 0).unwrap()),
            updated_at: None,
        }
    }

    #[test]
    fn tracker_tool_definitions_are_exposed_when_configured() {
        let definitions = tools().definitions();

        assert_eq!(definitions.len(), 1);
        assert_eq!(definitions[0]["name"], VIK_ISSUE_TOOL);
        assert_eq!(
            definitions[0].pointer("/inputSchema/required/0"),
            Some(&json!("action"))
        );
    }

    #[test]
    fn tracker_tool_definitions_are_hidden_without_tracker() {
        assert!(DynamicTools::default().definitions().is_empty());
    }

    #[test]
    fn app_server_tool_call_shape_is_extracted() {
        let params = json!({
            "tool": VIK_ISSUE_TOOL,
            "arguments": {
                "action": UPDATE_ISSUE_ACTION,
                "issue_id": "1",
                "state": "Done"
            }
        });
        assert_eq!(extract_tool_name(&params).as_deref(), Some(VIK_ISSUE_TOOL));
        assert_eq!(extract_tool_arguments(&params).unwrap()["state"], "Done");
    }

    #[test]
    fn string_arguments_are_parsed() {
        let params = json!({
            "tool": VIK_ISSUE_TOOL,
            "arguments": "{\"action\":\"update_issue\",\"issue_id\":\"1\",\"state\":\"Done\"}"
        });
        assert_eq!(extract_tool_arguments(&params).unwrap()["issue_id"], "1");
    }

    #[tokio::test]
    async fn update_issue_routes_to_configured_tracker() {
        let result = tools()
            .handle_call(&json!({
                "tool": VIK_ISSUE_TOOL,
                "arguments": {
                    "action": UPDATE_ISSUE_ACTION,
                    "issue_id": "42",
                    "state": "Done"
                }
            }))
            .await;

        assert_eq!(result["success"], true);
        let body: Value = serde_json::from_str(
            result
                .pointer("/contentItems/0/text")
                .unwrap()
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body["id"], "42");
        assert_eq!(body["state"], "Done");
    }

    #[tokio::test]
    async fn list_comments_routes_to_configured_tracker() {
        let result = tools()
            .handle_call(&json!({
                "tool": VIK_ISSUE_TOOL,
                "arguments": {
                    "action": LIST_COMMENTS_ACTION,
                    "issue_id": "42"
                }
            }))
            .await;

        assert_eq!(result["success"], true);
        let body: Value = serde_json::from_str(
            result
                .pointer("/contentItems/0/text")
                .unwrap()
                .as_str()
                .unwrap(),
        )
        .unwrap();
        assert_eq!(body[0]["id"], "comment-1");
        assert_eq!(body[0]["body"], "workpad for 42");
    }

    #[tokio::test]
    async fn missing_issue_id_returns_tool_failure_without_network() {
        let result = tools()
            .handle_call(&json!({
                "tool": VIK_ISSUE_TOOL,
                "arguments": { "action": UPDATE_ISSUE_ACTION, "state": "Done" }
            }))
            .await;

        assert_eq!(result["success"], false);
        assert_eq!(
            result.pointer("/contentItems/0/text"),
            Some(&json!("issue_id is required"))
        );
    }

    #[tokio::test]
    async fn upload_attachment_rejects_paths_outside_workspace() {
        let dir = tempdir().unwrap();
        let outside_dir = tempdir().unwrap();
        let outside_file = outside_dir.path().join("artifact.txt");
        std::fs::write(&outside_file, "artifact").unwrap();
        let tracker: Arc<dyn IssueTracker> = Arc::new(TestTracker);
        let tools = DynamicTools::from_tracker(tracker).with_workspace_root(dir.path());
        let result = tools
            .handle_call(&json!({
                "tool": VIK_ISSUE_TOOL,
                "arguments": {
                    "action": UPLOAD_ATTACHMENT_ACTION,
                    "issue_id": "42",
                    "path": outside_file.to_string_lossy(),
                    "content_type": "text/plain"
                }
            }))
            .await;

        assert_eq!(result["success"], false);
        assert_eq!(
            result.pointer("/contentItems/0/text"),
            Some(&json!(
                "attachment path must stay inside the issue workspace"
            ))
        );
    }
}
