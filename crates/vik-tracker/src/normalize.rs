use chrono::{DateTime, Utc};
use serde_json::Value;
use vik_core::{BlockerRef, Issue, normalize_state};

pub fn normalize_issue(node: &Value) -> Option<Issue> {
    let id = string_at(node, "/id")?;
    let identifier = string_at(node, "/identifier")?;
    let title = string_at(node, "/title")?;
    let state = string_at(node, "/state/name")?;
    let labels = node
        .pointer("/labels/nodes")
        .and_then(Value::as_array)
        .map(|labels| {
            labels
                .iter()
                .filter_map(|label| label.get("name").and_then(Value::as_str))
                .map(normalize_state)
                .collect()
        })
        .unwrap_or_default();
    let blocked_by = node
        .pointer("/inverseRelations/nodes")
        .and_then(Value::as_array)
        .map(|relations| {
            relations
                .iter()
                .filter(|relation| relation.get("type").and_then(Value::as_str) == Some("blocks"))
                .filter_map(|relation| {
                    let issue = relation.get("issue")?;
                    Some(BlockerRef {
                        id: issue
                            .get("id")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        identifier: issue
                            .get("identifier")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                        state: issue
                            .pointer("/state/name")
                            .and_then(Value::as_str)
                            .map(ToOwned::to_owned),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Some(Issue {
        id,
        identifier,
        title,
        description: opt_string(node, "description"),
        priority: node.get("priority").and_then(Value::as_i64),
        state,
        branch_name: opt_string(node, "branchName"),
        url: opt_string(node, "url"),
        labels,
        blocked_by,
        created_at: opt_datetime(node, "createdAt"),
        updated_at: opt_datetime(node, "updatedAt"),
    })
}

fn string_at(node: &Value, pointer: &str) -> Option<String> {
    node.pointer(pointer)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn opt_string(node: &Value, key: &str) -> Option<String> {
    node.get(key)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn opt_datetime(node: &Value, key: &str) -> Option<DateTime<Utc>> {
    node.get(key)
        .and_then(Value::as_str)
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .map(|value| value.with_timezone(&Utc))
}

pub fn dispatch_sort_key(issue: &Issue) -> (i64, Option<DateTime<Utc>>, String) {
    (
        issue.priority.unwrap_or(i64::MAX),
        issue.created_at,
        issue.identifier.clone(),
    )
}
