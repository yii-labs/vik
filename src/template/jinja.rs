use minijinja::{Environment, UndefinedBehavior};
use serde::Serialize;
use serde_json::{Map, Value};

use super::TemplateError;

#[derive(Debug, Clone)]
pub struct JinjaRenderer {
  context: Map<String, Value>,
}

impl JinjaRenderer {
  pub fn new() -> Self {
    let mut renderer = Self { context: Map::new() };
    renderer.with_envs(std::env::vars());

    renderer
  }

  /// Strict undefined behavior: an `{{ unknown_var }}` is an error
  /// rather than silently rendering as empty. This is what catches
  /// typos in operator-authored prompts and hooks at render time
  /// instead of at the next "why did this run with empty fields" report.
  pub fn render<Context: Serialize>(&self, template: &str, context: Context) -> Result<String, TemplateError> {
    // TODO: we should have a template registry to avoid compiling templates on every render
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    self.context.iter().for_each(|(k, v)| {
      env.add_global(k, minijinja::Value::from_serialize(v));
    });
    let template = env.template_from_str(template)?;
    Ok(template.render(context)?)
  }

  pub fn with_value<K: Into<String>, V: Serialize>(&mut self, key: K, value: V) -> &mut Self {
    match serde_json::to_value(&value) {
      Ok(v) => {
        self.context.insert(key.into(), v);
      },
      Err(e) => {
        let key = key.into();
        tracing::warn!(key = %&key, error = %e, "failed to serialize value for template context; returning null.");
        self.context.insert(key, Value::Null);
      },
    }

    self
  }

  pub fn with_values<K: Into<String>, V: Serialize, It: Iterator<Item = (K, V)>>(&mut self, iter: It) -> &mut Self {
    for (k, v) in iter {
      self.with_value(k, v);
    }

    self
  }

  pub fn with_envs<K: Into<String>, V: Into<String>, It: Iterator<Item = (K, V)>>(&mut self, iter: It) {
    let entry = self.context.entry("env");
    let value = entry.or_insert_with(|| Value::Object(Map::new()));

    if let Value::Object(map) = value {
      for (k, v) in iter {
        map.insert(k.into(), Value::String(v.into()));
      }
    }
  }

  pub fn with_env<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) -> &mut Self {
    self.with_envs(std::iter::once((key, value)));
    self
  }
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[test]
  fn render_with_default_envs() {
    unsafe {
      std::env::set_var("TEST_ENV_VAR", "1");
    }
    let renderer = JinjaRenderer::new();

    let rendered = renderer.render("{{ env.TEST_ENV_VAR }}", json!({})).expect("render");
    assert_eq!(rendered, "1");
  }

  #[test]
  fn render_with_added_env() {
    let mut renderer = JinjaRenderer::new();
    renderer.with_env("ADDED_ENV_VAR", "42");

    let rendered = renderer.render("{{ env.ADDED_ENV_VAR }}", json!({})).expect("render");
    assert_eq!(rendered, "42");
  }

  #[test]
  fn render_with_added_envs() {
    let mut renderer = JinjaRenderer::new();
    renderer.with_envs(vec![("VAR1", "value1"), ("VAR2", "value2")].into_iter());

    let rendered = renderer.render("{{ env.VAR1 }} {{ env.VAR2 }}", json!({})).expect("render");
    assert_eq!(rendered, "value1 value2");
  }

  #[test]
  fn render_with_added_value() {
    let mut renderer = JinjaRenderer::new();
    renderer.with_value("added_var", "hello");

    let rendered = renderer.render("{{ added_var }}", json!({})).expect("render");
    assert_eq!(rendered, "hello");
  }

  #[test]
  fn render_with_added_values() {
    let mut renderer = JinjaRenderer::new();
    renderer.with_values(vec![("var1", "value1"), ("var2", "value2")].into_iter());

    let rendered = renderer.render("{{ var1 }} {{ var2 }}", json!({})).expect("render");
    assert_eq!(rendered, "value1 value2");
  }

  #[test]
  fn render_with_passed_context() {
    let renderer = JinjaRenderer::new();

    let rendered = renderer
      .render("{{ passed_var }}", json!({ "passed_var": "passed value" }))
      .expect("render");
    assert_eq!(rendered, "passed value");
  }

  #[test]
  fn render_with_non_serializable_value() {
    let mut renderer = JinjaRenderer::new();
    renderer.with_value("bad_var", f64::NAN);

    let rendered = renderer.render("{{ bad_var }}", json!({})).expect("render");
    assert_eq!(rendered, "none");
  }

  #[test]
  fn render_with_unknown_variable_errors() {
    let renderer = JinjaRenderer::new();
    let err = renderer
      .render("{{ unknown_var }}", json!({}))
      .expect_err("render should fail with unknown variable");
    assert!(matches!(err, TemplateError::Render(e) if e.kind() == minijinja::ErrorKind::UndefinedError));
  }
}
