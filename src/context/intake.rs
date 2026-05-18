use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

/// Aliases match the field names trackers commonly emit: Linear uses
/// `identifier`/`description`, GitHub uses `id`/`desc` in different
/// places, etc. Keeping both forms accepted means workflow authors do
/// not have to write a transformation step in their pull command.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Issue {
  #[serde(alias = "identifier")]
  pub id: String,
  pub title: String,
  #[serde(alias = "desc", default = "String::new")]
  pub description: String,
  #[serde(alias = "status")]
  pub state: String,

  /// Captures any tracker-specific fields. The session renderer
  /// flattens these onto the prompt context so workflow authors can
  /// reach for `{{ priority }}` / `{{ labels }}` / etc. without us
  /// having to model every tracker shape.
  #[serde(flatten)]
  pub extra_payload: serde_yaml::Mapping,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Issues(Vec<Issue>);

impl Deref for Issues {
  type Target = Vec<Issue>;

  fn deref(&self) -> &Self::Target {
    &self.0
  }
}

impl DerefMut for Issues {
  fn deref_mut(&mut self) -> &mut Self::Target {
    &mut self.0
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn issue_defaults_description_to_empty_string() {
    let issue: Issue = serde_json::from_value(json!({
      "id": "ISSUE-1",
      "title": "Default description",
      "state": "Todo"
    }))
    .expect("issue deserializes");

    assert_eq!(issue.description, "");
  }

  #[test]
  fn issue_deserializes_tracker_aliases() {
    let issue: Issue = serde_json::from_value(json!({
      "identifier": "LIN-1",
      "title": "Alias issue",
      "desc": "Alias description",
      "status": "Work"
    }))
    .expect("issue deserializes");

    assert_eq!(issue.id, "LIN-1");
    assert_eq!(issue.description, "Alias description");
    assert_eq!(issue.state, "Work");
  }

  #[test]
  fn issue_keeps_extra_payload_flattened() {
    let issue: Issue = serde_json::from_value(json!({
      "id": "ISSUE-2",
      "title": "Extra payload",
      "state": "Todo",
      "priority": "high",
      "labels": ["bug", "ui"]
    }))
    .expect("issue deserializes");

    let priority_key = serde_yaml::Value::String("priority".into());
    assert_eq!(issue.extra_payload.len(), 2);
    assert_eq!(
      issue.extra_payload.get(&priority_key),
      Some(&serde_yaml::Value::String("high".into()))
    );

    let serialized = serde_json::to_value(&issue).expect("issue serializes");
    assert_eq!(serialized["priority"], json!("high"));
    assert_eq!(serialized["labels"], json!(["bug", "ui"]));
    assert!(serialized.get("extra_payload").is_none());
  }

  #[test]
  fn issues_deserializes_array_and_mutates_like_vec() {
    let mut issues: Issues = serde_json::from_value(json!([
      {
        "id": "ISSUE-3",
        "title": "First issue",
        "state": "Todo"
      }
    ]))
    .expect("issues deserialize");

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].id, "ISSUE-3");

    issues.push(issue("ISSUE-4"));

    let ids: Vec<_> = issues.iter().map(|issue| issue.id.as_str()).collect();
    assert_eq!(ids, ["ISSUE-3", "ISSUE-4"]);
  }

  fn issue(id: &str) -> Issue {
    Issue {
      id: id.to_string(),
      title: "Added issue".to_string(),
      description: String::new(),
      state: "Work".to_string(),
      extra_payload: serde_yaml::Mapping::new(),
    }
  }
}
