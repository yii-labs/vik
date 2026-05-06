use serde_json::{Map, json};

use crate::providers::TrackerConfigError;

use super::{
    FeishuTrackerConfig,
    client::record_fields_from_get_payload,
    client::{
        FeishuIssueFields, FeishuRecord, issue_from_record, issue_from_record_with_view_id,
        records_from_list_payload,
    },
};

fn issue_fields() -> FeishuIssueFields {
    FeishuIssueFields {
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "PR Links".to_string(),
    }
}

#[cfg(unix)]
fn write_executable_script(prefix: &str, script: &str) -> (std::path::PathBuf, std::path::PathBuf) {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("{prefix}-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    let cli_path = dir.join("lark-cli");
    fs::write(&cli_path, script).unwrap();
    let mut permissions = fs::metadata(&cli_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cli_path, permissions).unwrap();
    (dir, cli_path)
}

#[test]
fn provider_config_owns_cli_base_table_and_validation() {
    let config = FeishuTrackerConfig::new("base_token", "tbl123");

    assert_eq!(config.cli_path, "lark-cli");
    assert_eq!(config.base_token, "base_token");
    assert_eq!(config.table_id, "tbl123");
    assert_eq!(config.view_id, "");
    assert_eq!(config.identity, "user");
    assert_eq!(config.fields_map.description, "");
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
        title: "Text".to_string(),
        description: "Description".to_string(),
        state: "State".to_string(),
        labels: "Labels".to_string(),
        comments: "Workpad".to_string(),
        pr_links: "Workpad".to_string(),
    };

    assert_eq!(
        fields.names(),
        vec!["Text", "State", "Description", "Labels", "Workpad",]
    );
}

#[test]
fn parses_record_list_payload_with_record_ids() {
    let payload = json!({
        "ok": true,
        "data": {
            "fields": ["Text", "State", "Labels"],
            "record_id_list": ["rec1", "rec2"],
            "data": [
                ["Issue one", ["Todo"], ["feature"]],
                ["Issue two", ["Done"], null]
            ]
        }
    });

    let records = records_from_list_payload(&payload).unwrap();

    assert_eq!(records.len(), 2);
    assert_eq!(records[0].id, "rec1");
    assert_eq!(records[0].fields["Labels"], json!(["feature"]));
    assert_eq!(records[1].fields["Labels"], json!(null));
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
    let fields = issue_fields();
    let record = FeishuRecord {
        id: "rec123".to_string(),
        fields: Map::from_iter([
            ("Text".to_string(), json!("Add Feishu tracker")),
            ("Description".to_string(), json!("Use Base as tracker")),
            ("State".to_string(), json!(["Todo"])),
            ("Labels".to_string(), json!(["feature", "backend"])),
        ]),
    };

    let issue = issue_from_record(&record, &fields);

    assert_eq!(issue.id, "rec123");
    assert_eq!(issue.identifier, "rec123");
    assert_eq!(issue.title, "Add Feishu tracker");
    assert_eq!(issue.state, "Todo");
    assert_eq!(issue.labels, vec!["feature", "backend"]);
}

#[test]
fn uses_record_id_as_identifier() {
    let fields = issue_fields();
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
fn normalizes_issue_url_without_view_id() {
    let fields = issue_fields();
    let record = FeishuRecord {
        id: "rec123".to_string(),
        fields: Map::from_iter([
            ("Text".to_string(), json!("Add Feishu tracker")),
            ("State".to_string(), json!(["Todo"])),
        ]),
    };

    let issue = issue_from_record(&record, &fields);

    assert_eq!(
        issue.url.as_deref(),
        Some("https://www.feishu.cn/base/base?table=table")
    );
}

#[test]
fn normalizes_issue_url_with_view_id() {
    let fields = issue_fields();
    let record = FeishuRecord {
        id: "rec123".to_string(),
        fields: Map::from_iter([
            ("Text".to_string(), json!("Add Feishu tracker")),
            ("State".to_string(), json!(["Todo"])),
        ]),
    };

    let issue = issue_from_record_with_view_id(&record, &fields, " vewpBV8AK0 ");

    assert_eq!(
        issue.url.as_deref(),
        Some("https://www.feishu.cn/base/base?table=table&view=vewpBV8AK0")
    );
}

#[tokio::test]
async fn empty_active_states_match_no_candidates_without_listing_records() {
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let config = FeishuClientConfig::new(
        "missing-lark-cli",
        "base",
        "table",
        "user",
        Vec::new(),
        issue_fields(),
    );
    let client = FeishuClient::new(config).unwrap();

    let issues = client.fetch_candidates().await.unwrap();

    assert!(issues.is_empty());
}

#[tokio::test]
async fn empty_state_filter_matches_no_records_without_listing_records() {
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let config = FeishuClientConfig::new(
        "missing-lark-cli",
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        issue_fields(),
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
    "fields": ["Text", "State"],
    "record_id_list": ["rec1"],
    "data": [
      ["Issue one", ["Todo"]]
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

    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        issue_fields(),
    )
    .with_view_id(" vewpBV8AK0 ");
    let client = FeishuClient::new(config).unwrap();

    let issues = client.fetch_candidates().await.unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].identifier, "rec1");
    assert_eq!(
        issues[0].url.as_deref(),
        Some("https://www.feishu.cn/base/base?table=table&view=vewpBV8AK0")
    );

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
    "fields": ["Text", "State", "Labels", "Workpad", "PR Links"],
    "record_id_list": ["rec1", "rec2"],
    "data": [
      ["Issue one", ["Todo"], ["feature"], null, null],
      ["Issue two", ["Todo"], ["other"], null, null]
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

    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        issue_fields(),
    )
    .with_filter_tags(vec!["feature".to_string()]);
    let client = FeishuClient::new(config).unwrap();

    let issues = client.fetch_candidates().await.unwrap();

    assert_eq!(issues.len(), 1);
    assert_eq!(issues[0].identifier, "rec1");
    assert_eq!(
        issues[0].url.as_deref(),
        Some("https://www.feishu.cn/base/base?table=table")
    );

    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[tokio::test]
async fn update_issue_writes_labels_as_multi_select_array() {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::time::{SystemTime, UNIX_EPOCH};
    use vik_core::{IssueTracker, IssueUpdate};

    use super::client::{FeishuClient, FeishuClientConfig};

    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let dir = std::env::temp_dir().join(format!("vik-feishu-label-test-{unique}"));
    fs::create_dir_all(&dir).unwrap();
    let cli_path = dir.join("lark-cli");
    fs::write(
        &cli_path,
        r#"#!/bin/sh
command=""
json_arg=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    +record-get|+record-upsert)
      command="$1"
      ;;
    --json)
      json_arg="${2:-}"
      shift
      ;;
  esac
  shift
done
if [ "$command" = "+record-upsert" ]; then
  case "$json_arg" in
    *'"Labels":["feature","backend"]'*)
      printf '{"ok":true,"data":{"record":{}}}\n'
      exit 0
      ;;
    *)
      echo "unexpected labels payload: $json_arg" >&2
      exit 7
      ;;
  esac
fi
cat <<'JSON'
{
  "ok": true,
  "data": {
    "record": {
      "fields": {
        "Text": "Issue one",
        "State": ["Todo"],
        "Labels": ["feature"]
      }
    }
  }
}
JSON
"#,
    )
    .unwrap();
    let mut permissions = fs::metadata(&cli_path).unwrap().permissions();
    permissions.set_mode(0o755);
    fs::set_permissions(&cli_path, permissions).unwrap();

    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        issue_fields(),
    );
    let client = FeishuClient::new(config).unwrap();

    let issue = client
        .update_issue(
            "rec1",
            IssueUpdate {
                state: None,
                labels: vec!["backend".to_string()],
            },
        )
        .await
        .unwrap();

    assert_eq!(issue.identifier, "rec1");

    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[tokio::test]
async fn create_comment_writes_plain_text_workpad() {
    use std::fs;
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let (dir, cli_path) = write_executable_script(
        "vik-feishu-workpad-create-test",
        r#"#!/bin/sh
command=""
json_arg=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    +record-get|+record-upsert)
      command="$1"
      ;;
    --json)
      json_arg="${2:-}"
      shift
      ;;
  esac
  shift
done
if [ "$command" = "+record-upsert" ]; then
  case "$json_arg" in
    *'"Workpad":"Codex workpad body"'*)
      printf '{"ok":true,"data":{"record":{}}}\n'
      exit 0
      ;;
    *)
      echo "unexpected workpad payload: $json_arg" >&2
      exit 7
      ;;
  esac
fi
cat <<'JSON'
{
  "ok": true,
  "data": {
    "record": {
      "fields": {
        "Text": "Issue one",
        "State": ["Todo"],
        "Workpad": ""
      }
    }
  }
}
JSON
"#,
    );
    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        issue_fields(),
    );
    let client = FeishuClient::new(config).unwrap();

    let comment = client
        .create_comment("rec1", "Codex workpad body")
        .await
        .unwrap();

    assert_eq!(comment.id, "rec1:workpad");
    assert_eq!(comment.body, "Codex workpad body");
    assert_eq!(comment.url, None);

    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[tokio::test]
async fn list_comments_reads_plain_text_workpad() {
    use std::fs;
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let (dir, cli_path) = write_executable_script(
        "vik-feishu-workpad-list-test",
        r#"#!/bin/sh
cat <<'JSON'
{
  "ok": true,
  "data": {
    "record": {
      "fields": {
        "Text": "Issue one",
        "State": ["Todo"],
        "Workpad": "  keep space\nLine two  "
      }
    }
  }
}
JSON
"#,
    );
    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        issue_fields(),
    );
    let client = FeishuClient::new(config).unwrap();

    let comments = client.list_comments("rec1").await.unwrap();

    assert_eq!(comments.len(), 1);
    assert_eq!(comments[0].id, "rec1:workpad");
    assert_eq!(comments[0].body, "  keep space\nLine two  ");
    assert_eq!(comments[0].url, None);

    let _ = fs::remove_dir_all(dir);
}

#[cfg(unix)]
#[tokio::test]
async fn update_comment_writes_plain_text_workpad() {
    use std::fs;
    use vik_core::IssueTracker;

    use super::client::{FeishuClient, FeishuClientConfig};

    let (dir, cli_path) = write_executable_script(
        "vik-feishu-workpad-update-test",
        r#"#!/bin/sh
command=""
json_arg=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    +record-get|+record-upsert)
      command="$1"
      ;;
    --json)
      json_arg="${2:-}"
      shift
      ;;
  esac
  shift
done
if [ "$command" = "+record-upsert" ]; then
  case "$json_arg" in
    *'"Workpad":"Updated workpad body"'*)
      printf '{"ok":true,"data":{"record":{}}}\n'
      exit 0
      ;;
    *)
      echo "unexpected workpad payload: $json_arg" >&2
      exit 7
      ;;
  esac
fi
cat <<'JSON'
{
  "ok": true,
  "data": {
    "record": {
      "fields": {
        "Text": "Issue one",
        "State": ["Todo"],
        "Workpad": "Existing workpad body"
      }
    }
  }
}
JSON
"#,
    );
    let config = FeishuClientConfig::new(
        cli_path.to_string_lossy().to_string(),
        "base",
        "table",
        "user",
        vec!["Todo".to_string()],
        issue_fields(),
    );
    let client = FeishuClient::new(config).unwrap();

    let comment = client
        .update_comment("rec1:workpad", "Updated workpad body")
        .await
        .unwrap();

    assert_eq!(comment.id, "rec1:workpad");
    assert_eq!(comment.body, "Updated workpad body");
    assert_eq!(comment.url, None);

    let _ = fs::remove_dir_all(dir);
}
