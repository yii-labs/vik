mod intake;
mod run;

use serde::Serialize;
use serde_json::{Map, Value};

pub use intake::{Issue, Issues};
pub use run::{IssueRun, IssueStage, IssueStageKey};

pub struct Context {
  inner: Map<String, Value>,
}

impl Serialize for Context {
  fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
    self.inner.serialize(serializer)
  }
}

impl Context {
  pub fn new() -> Self {
    Self { inner: Map::new() }
  }

  pub fn with_value<K: Into<String>, V: Into<Value>>(&mut self, key: K, value: V) -> &mut Self {
    self.inner.insert(key.into(), value.into());
    self
  }

  pub fn with_values<K: Into<String>, V: Into<Value>>(
    &mut self,
    entries: impl IntoIterator<Item = (K, V)>,
  ) -> &mut Self {
    for (key, value) in entries {
      self.with_value(key, value);
    }
    self
  }
}

pub trait RenderContext {
  fn as_render_context(&self) -> Context;
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn context_serializes_values() {
    let mut context = Context::new();
    context
      .with_value("string", "value")
      .with_value("number", 42)
      .with_value("object", json!({"key": "value"}));

    let serialized = serde_json::to_string(&context.inner).expect("context serializes");
    assert_eq!(serialized, r#"{"number":42,"object":{"key":"value"},"string":"value"}"#);
  }

  #[test]
  fn context_with_values_allows_chaining() {
    let mut context = Context::new();
    context.with_values([("key1", "val1"), ("key2", "val2")]);

    let serialized = serde_json::to_string(&context.inner).expect("context serializes");
    assert_eq!(serialized, r#"{"key1":"val1","key2":"val2"}"#);
  }
}
