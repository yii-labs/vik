use serde_json::{Map, json};

use crate::providers::TrackerConfigError;

use super::{
    FeishuTrackerConfig,
    client::record_fields_from_get_payload,
    client::{FeishuIssueFields, FeishuRecord, issue_from_record, records_from_list_payload},
};

#[test]
fn provider_config_owns_cli_base_table_and_validation() {
    let config = FeishuTrackerConfig::new("base_token", "tbl123");

    assert_eq!(config.cli_path, "lark-cli");
    assert_eq!(config.base_token, "base_token");
    assert_eq!(config.table_id, "tbl123");
    assert_eq!(config.view_id, "");
    assert_eq!(config.identity, "user");
    assert_eq!(config.fields_map.identifier, "");
    assert_eq!(config.fields_map.description, "");
    assert_eq!(config.fields_map.delegated, "");
    assert_eq!(config.fields_map.title, "Title");
    assert_eq!(config.fields_map.comments, "Workpad");
    assert_eq!(config.fields_map.pr_links, "PR Links");
    config.validate().unwrap();

    let missing_base = FeishuTrackerConfig::new("", "tbl123");
    assert!(matches!(
        missing_base.validate(),
        Err(TrackerConfigError::MissingBaseToken)
    ));

    let missing_table = FeishuTrackerConfig::new("base_token", "");
    assert!(matches!(
        missing_table.validate(),
        Err(TrackerConfigError::MissingTableId)
    ));

    let mut invalid_identity = FeishuTrackerConfig::new("base_token", "tbl123");
    invalid_identity.identity = "service".to_string();
    assert!(matches!(
        invalid_identity.validate(),
        Err(TrackerConfigError::InvalidCliIdentity(identity)) if identity == "service"
    ));
}

#[test]
fn field_names_are_deduplicated_and_skip_empty_values() {
    let fields = FeishuIssueFields {
        identifier: "Identifier".to_string(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "Workpad".to_string(),
    };

    assert_eq!(
        fields.names(),
        vec![
            "Text",
            "Identifier",
            "State",
            "Description",
            "Labels",
            "Workpad",
            "AI Delegated"
        ]
    );
}

#[test]
fn parses_record_list_payload_with_record_ids() {
    let payload = json!({
        "ok": true,
        "data": {
            "fields": ["Text", "Identifier", "State"],
            "record_id_list": ["rec1", "rec2"],
            "data": [
                ["Issue one", "VIK-1", ["Todo"]],
                ["Issue two", null, ["Done"]]
            ]
        }
    });

    let records = records_from_list_payload(&payload).unwrap();

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, "rec1");
    assert_eq!(records[0].fields["Identifier"], json!("VIK-1"));
    assert_eq!(records[1].fields["Identifier"], json!(null));
}

#[test]
fn parses_record_get_payload_with_flattened_fields() {
    let payload = json!({
        "ok": true,
        "data": {
            "record": {
                "Text": "Issue one",
                "State": ["Todo"]
            }
        }
    });

    let fields = record_fields_from_get_payload(&payload).unwrap();

    assert_eq!(fields["Text"], json!("Issue one"));
    assert_eq!(fields["State"], json!(["Todo"]));
}

#[test]
fn parses_record_get_payload_with_nested_fields() {
    let payload = json!({
        "ok": true,
        "data": {
            "record": {
                "record_id": "rec1",
                "fields": {
                    "Text": "Issue one",
                    "State": ["Todo"]
                }
            }
        }
    });

    let fields = record_fields_from_get_payload(&payload).unwrap();

    assert_eq!(fields["Text"], json!("Issue one"));
    assert_eq!(fields["State"], json!(["Todo"]));
    assert!(fields.get("record_id").is_none());
}

#[test]
fn normalizes_issue_fields_and_label_text() {
    let fields = FeishuIssueFields {
        identifier: "Identifier".to_string(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    };
    let record = FeishuRecord {
        id: "rec123".to_string(),
        fields: Map::from_iter([
            ("Identifier".to_string(), json!("VIK-123")),
            ("Text".to_string(), json!("Add Feishu tracker")),
            ("Description".to_string(), json!("Use Base as tracker")),
            ("State".to_string(), json!(["Todo"])),
            ("Labels".to_string(), json!("feature, backend")),
        ]),
    };

    let issue = issue_from_record(&record, &fields);

    assert_eq!(issue.id, "rec123");
    assert_eq!(issue.identifier, "VIK-123");
    assert_eq!(issue.title, "Add Feishu tracker");
    assert_eq!(issue.state, "Todo");
    assert_eq!(issue.labels, vec!["feature", "backend"]);
}

#[test]
fn falls_back_to_record_id_for_missing_identifier() {
    let fields = FeishuIssueFields {
        identifier: "Identifier".to_string(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    };
    let record = FeishuRecord {
        id: "rec123".to_string(),
        fields: Map::from_iter([
            ("Text".to_string(), json!("Add Feishu tracker")),
            ("State".to_string(), json!(["Todo"])),
        ]),
    };

    let issue = issue_from_record(&record, &fields);

    assert_eq!(issue.identifier, "rec123");
}

#[test]
fn uses_record_id_when_identifier_field_is_not_configured() {
    let fields = FeishuIssueFields {
        identifier: String::new(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    };
    let record = FeishuRecord {
        id: "rec123".to_string(),
        fields: Map::from_iter([
            ("Text".to_string(), json!("Add Feishu tracker")),
            ("Identifier".to_string(), json!("VIK-123")),
            ("State".to_string(), json!(["Todo"])),
        ]),
    };

    let issue = issue_from_record(&record, &fields);

    assert_eq!(issue.id, "rec123");
    assert_eq!(issue.identifier, "rec123");
}

#[cfg(unix)]
#[tokio::test]
async fn get_issue_uses_exact_identifier_match_from_search_results() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("vik-feishu-search-test-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    let cli_path = dir.join("lark-cli");
    fs::write(
        &cli_path,
        r#"#!/bin/sh
cat <<'JSON'
{
  "ok": true,
  "data": {
    "fields": ["Text", "Identifier", "State"],
    "record_id_list": ["rec10", "rec1"],
    "data": [
      ["Issue ten", "VIK-10", ["Todo"]],
      ["Issue one", "VIK-1", ["Todo"]]
    ],
    "has_more": false
  }
}
JSON
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&cli_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cli_path, permissions).unwrap();

    let fields = FeishuIssueFields {
        identifier: "Identifier".to_string(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    };
    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        fields,
    );
    let client = FeishuClient::new(config).unwrap();

    let issue = client.get_issue("VIK-1").await.unwrap();

    assert_eq!(issue.id, "rec1");
    assert_eq!(issue.identifier, "VIK-1");

    let _ = fs::remove_dir_all(dir);
}

#[tokio::test]
async fn empty_active_states_match_no_candidates_without_listing_records() {
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let fields = FeishuIssueFields {
        identifier: String::new(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    };
    let config = FeishuClientConfig::new(
        "missing-lark-cli",
        "base",
        "table",
        "user",
        Vec::new(),
        fields,
    );
    let client = FeishuClient::new(config).unwrap();

    let issues = client.fetch_candidates().await.unwrap();

    assert!(issues.is_empty());
}

#[tokio::test]
async fn empty_state_filter_matches_no_records_without_listing_records() {
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let fields = FeishuIssueFields {
        identifier: String::new(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    };
    let config = FeishuClientConfig::new(
        "missing-lark-cli",
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        fields,
    );
    let client = FeishuClient::new(config).unwrap();

    let issues = client.fetch_by_states(&[]).await.unwrap();

    assert!(issues.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn fetch_candidates_passes_configured_view_id_to_record_list() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("vik-feishu-view-test-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    let cli_path = dir.join("lark-cli");
    fs::write(
        &cli_path,
        r#"#!/bin/sh
found=0
while [ "$#" -gt 0 ]; do
  if [ "$1" = "--view-id" ] && [ "${2:-}" = "vewpBV8AK0" ]; then
    found=1
  fi
  shift
done
if [ "$found" -ne 1 ]; then
  echo "missing --view-id vewpBV8AK0" >&2
  exit 7
fi
cat <<'JSON'
{
  "ok": true,
  "data": {
    "fields": ["Text", "Identifier", "State", "AI Delegated"],
    "record_id_list": ["rec1"],
    "data": [
      ["Issue one", "VIK-1", ["Todo"], true]
    ],
    "has_more": false
  }
}
JSON
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&cli_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cli_path, permissions).unwrap();

    let fields = FeishuIssueFields {
        identifier: "Identifier".to_string(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    };
    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        fields,
    )
    .with_view_id(" vewpBV8AK0 ");
    let client = FeishuClient::new(config).unwrap();

    let issues = client.fetch_candidates().await.unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].identifier, "VIK-1");

    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[tokio::test]
async fn fetch_candidates_routes_through_lark_cli() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("vik-feishu-test-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    let cli_path = dir.join("lark-cli");
    fs::write(
        &cli_path,
        r#"#!/bin/sh
cat <<'JSON'
{
  "ok": true,
  "data": {
    "fields": ["Text", "Identifier", "State", "AI Delegated", "Labels", "Workpad", "PR Links"],
    "record_id_list": ["rec1", "rec2"],
    "data": [
      ["Issue one", "VIK-1", ["Todo"], true, "feature", null, null],
      ["Issue two", "VIK-2", ["Todo"], false, "feature", null, null]
    ],
    "has_more": false
  }
}
JSON
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&cli_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cli_path, permissions).unwrap();

    let fields = FeishuIssueFields {
        identifier: "Identifier".to_string(),
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        delegated: "AI Delegated".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    };
    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        fields,
    )
    .with_filter_tags(vec!["feature".to_string()]);
    let client = FeishuClient::new(config).unwrap();

    let issues = client.fetch_candidates().await.unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].identifier, "VIK-1");

    let _ = fs::remove_dir_all(dir);
}
