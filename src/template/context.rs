use std::path::Path;

use minijinja::Value;
use serde::Serialize;
use serde_json::{Map, Value as JsonValue};

use crate::context::Issue;

pub struct Context {
  inner: Map<String, JsonValue>,
}

impl Context {
  /// Seeds `env` with the Vik process environment so templates can
  /// read `{{ env.VAR }}` without operators having to thread
  /// individual variables through.
  pub fn new() -> Self {
    let inner = serde_json::Map::new();
    let mut context = Self { inner };
    context.with_envs(std::env::vars());
    context
  }

  pub fn with<K: Into<String>, V: Serialize>(&mut self, key: K, value: V) -> &mut Self {
    self.inner.insert(
      key.into(),
      serde_json::to_value(value).unwrap_or(serde_json::Value::Null),
    );
    self
  }

  /// ```
  /// let base_context = Context::new().with("key1", "value1");
  ///
  /// let derived_context = base_context.derive();
  /// derived_context.with("key2", "value2");
  ///
  /// assert_eq!(base_context.build().get("key1").unwrap(), "value1");
  /// assert!(base_context.build().get("key2").is_none());
  ///
  /// assert_eq!(derived_context.build().get("key1").unwrap(), "value1");
  /// assert_eq!(derived_context.build().get("key2").unwrap(), "value2");
  /// ```
  #[allow(dead_code)]
  pub fn derive(&self) -> Self {
    Self {
      inner: self.inner.clone(),
    }
  }

  pub fn build(&self) -> Value {
    Value::from_serialize(&self.inner)
  }

  pub fn with_envs<It: Iterator<Item = (String, String)>>(&mut self, iter: It) {
    let entry = self.inner.entry("env");
    let value = entry.or_insert_with(|| JsonValue::Object(Map::new()));
    if let JsonValue::Object(map) = value {
      for (k, v) in iter {
        map.insert(k, JsonValue::String(v));
      }
    }
  }
}

/// Per-stage template inputs shared by hook and prompt renderers, so
/// the two surfaces cannot drift on what `cwd`/`workspace`/`issue`/
/// `stage` mean. Fields borrow from whatever the caller already holds —
/// no cloning forced here.
pub struct StageContext<'a> {
  pub issue: &'a Issue,
  pub stage_name: &'a str,
  pub agent_profile: &'a str,
  pub stage_state: &'a str,
  pub issue_workdir: &'a Path,
  pub workspace_root: &'a Path,
}

impl<'a> StageContext<'a> {
  /// Writes the canonical stage bindings on top of an existing
  /// context. The session renderer flattens issue payload first and
  /// then calls this so payload keys cannot shadow `cwd`/`stage`/etc.
  pub fn apply(&self, context: &mut Context) {
    context.with("cwd", self.issue_workdir.to_string_lossy().as_ref());
    context.with(
      "workspace",
      serde_json::json!({ "root": self.workspace_root.to_string_lossy() }),
    );
    context.with("issue", issue_value(self.issue, self.issue_workdir));
    context.with(
      "stage",
      serde_json::json!({
        "name": self.stage_name,
        "agent": self.agent_profile,
        "state": self.stage_state,
      }),
    );
  }

  pub fn build_template_context(&self) -> Context {
    let mut context = Context::new();
    self.apply(&mut context);
    context
  }

  pub fn build_template_context_with_cwd(&self) -> (Context, std::path::PathBuf) {
    (self.build_template_context(), self.issue_workdir.to_path_buf())
  }
}

/// `identifier` is added as a friendly alias for `id`, and `workdir`
/// only when a non-empty path is supplied — the `after_create` hook
/// fires before a stage workdir is bound, and we do not want operators
/// to reach for `issue.workdir` at that point in the lifecycle.
pub fn issue_value(issue: &Issue, issue_workdir: &Path) -> serde_json::Value {
  let mut value = serde_json::to_value(issue).unwrap_or(serde_json::Value::Null);
  if let serde_json::Value::Object(map) = &mut value {
    map.insert("identifier".to_string(), serde_json::Value::String(issue.id.clone()));
    let workdir = issue_workdir.to_string_lossy().into_owned();
    if !workdir.is_empty() {
      map
        .entry("workdir".to_string())
        .or_insert_with(|| serde_json::Value::String(workdir));
    }
  }
  value
}
