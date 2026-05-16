use std::ops::{Deref, DerefMut};

use serde::{Deserialize, Serialize};

/// Aliases match the field names trackers commonly emit: Linear uses
/// `identifier`/`description`, GitHub uses `id`/`desc` in different
/// places, etc. Keeping both forms accepted means workflow authors do
/// not have to write a transformation step in their pull command.
#[derive(Debug, Clone, Serialize, Deserialize)]
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
