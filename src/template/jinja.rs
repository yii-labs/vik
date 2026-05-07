use minijinja::{Environment, UndefinedBehavior};
use serde::Serialize;

use super::TemplateError;

#[derive(Debug, Clone)]
pub struct JinjaRenderer {
  context: serde_json::Map<String, serde_json::Value>,
}

impl JinjaRenderer {
  pub fn new() -> Self {
    Self {
      context: serde_json::Map::new(),
    }
  }

  /// Strict undefined behavior: an `{{ unknown_var }}` is an error
  /// rather than silently rendering as empty. This is what catches
  /// typos in operator-authored prompts and hooks at render time
  /// instead of at the next "why did this run with empty fields" report.
  pub fn render<Context: Serialize>(&self, template: &str, context: &Context) -> Result<String, TemplateError> {
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    self.context.iter().for_each(|(k, v)| {
      env.add_global(k, minijinja::Value::from_serialize(v));
    });
    let template = env.template_from_str(template)?;
    Ok(template.render(context)?)
  }
}
