//! Validation framework shared across every config sub-schema.
//!
//! Each schema implements [`Diagnose`] and accumulates [`Diagnostic`]
//! findings into a [`Diagnostics`] bag. Validators receive the full
//! [`WorkflowSchema`] so a child can validate cross-references (the
//! prime example: a stage's `agent` must exist in the top-level
//! `agents` map). Pointers are dotted paths composed by
//! [`Diagnostics::extends_with_pointer`] as the validator walks the tree.

use std::fmt::Display;

use serde::Serialize;

use crate::config::WorkflowSchema;

macro_rules! diagnose_fields {
  ($diagnostics:ident, $receiver:tt, $schema:expr, $( $pointer:literal => $field:ident ),+ $(,)?) => {
    $(
      $diagnostics.extends_with_pointer($pointer, $receiver.$field.diagnose($schema));
    )+
  };
}

pub(crate) use diagnose_fields;

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostics {
  pub errors: Vec<Diagnostic>,
  pub warnings: Vec<Diagnostic>,
}

impl Display for Diagnostics {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    for diag in self.errors.iter().chain(self.warnings.iter()) {
      writeln!(f, "{}", diag)?;
    }
    Ok(())
  }
}

impl Diagnostics {
  pub fn new() -> Self {
    Self {
      errors: Vec::new(),
      warnings: Vec::new(),
    }
  }

  pub fn extends_with_pointer(&mut self, pointer: &str, other: Diagnostics) {
    self.errors.extend(other.errors.into_iter().map(|diag| {
      let extended_pointer = extend_pointer(pointer, &diag.pointer);
      diag.with_pointer(extended_pointer)
    }));
    self.warnings.extend(other.warnings.into_iter().map(|diag| {
      let extended_pointer = extend_pointer(pointer, &diag.pointer);
      diag.with_pointer(extended_pointer)
    }));
  }

  pub fn has_errors(&self) -> bool {
    !self.errors.is_empty()
  }

  pub fn has_warnings(&self) -> bool {
    !self.warnings.is_empty()
  }

  pub fn push(&mut self, diag: Diagnostic) {
    match diag.severity {
      DiagnosticSeverity::Error => self.errors.push(diag),
      DiagnosticSeverity::Warning => self.warnings.push(diag),
    }
  }

  pub fn error_if_empty_str(&mut self, pointer: &str, value: &str) {
    if value.trim().is_empty() {
      self.push(Diagnostic::error(pointer, DiagnosticCode::EmptyStr));
    }
  }

  pub fn error_if_empty_map(&mut self, pointer: &str, is_empty: bool) {
    if is_empty {
      self.push(Diagnostic::error(pointer, DiagnosticCode::EmptyMap));
    }
  }

  pub fn error_if_empty_map_here(&mut self, is_empty: bool) {
    self.error_if_empty_map("", is_empty);
  }

  pub fn error_if_non_positive(&mut self, pointer: &str, value: usize) {
    if value == 0 {
      self.push(Diagnostic::error(pointer, DiagnosticCode::NonPositiveNumber(value)));
    }
  }

  pub fn warn_unknown_fields(&mut self, fields: &serde_yaml::Mapping) {
    self.extend(
      fields
        .keys()
        .filter_map(|key| key.as_str().map(|key| Diagnostic::warning(key, DiagnosticCode::UnknownField))),
    );
  }

  pub fn error_if_empty_path(&mut self, pointer: &str, path: &std::path::Path) {
    if path.as_os_str().is_empty() {
      self.push(Diagnostic::error(pointer, DiagnosticCode::EmptyStr));
    }
  }

  pub fn extend<I: IntoIterator<Item = Diagnostic>>(&mut self, diags: I) {
    for diag in diags {
      self.push(diag);
    }
  }
}

/// Compose parent and child pointer segments without producing
/// `parent.` or `.child` artifacts when one side is empty — an empty
/// segment means "this level," not "an empty key."
fn extend_pointer(parent: &str, child: &str) -> String {
  match (parent.is_empty(), child.is_empty()) {
    (true, true) => String::new(),
    (true, false) => child.to_string(),
    (false, true) => parent.to_string(),
    (false, false) => format!("{parent}.{child}"),
  }
}

#[derive(Debug, Clone, Serialize)]
pub struct Diagnostic {
  pub severity: DiagnosticSeverity,
  /// Dotted path to the offending field, built up as the validator
  /// walks the tree. Empty string means "the schema root."
  pub pointer: String,
  /// Machine-stable enum so `vik doctor --json` consumers can filter
  /// without parsing English messages.
  pub code: DiagnosticCode,
}

#[derive(Debug, Clone, Copy, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticSeverity {
  Error,
  Warning,
}

impl Display for DiagnosticSeverity {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    write!(
      f,
      "{}",
      match self {
        DiagnosticSeverity::Error => "error",
        DiagnosticSeverity::Warning => "warning",
      }
    )
  }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DiagnosticCode {
  NonPositiveNumber(usize),
  UnknownField,
  EmptyStr,
  EmptyMap,
  UnknownAgent(String),
  DuplicateStageName(String),
}

impl Display for Diagnostic {
  fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
    match &self.code {
      DiagnosticCode::NonPositiveNumber(num) => {
        write!(f, "'{}' expected to be great than 0, got {}", self.pointer, num)
      },
      DiagnosticCode::UnknownField => write!(f, "unknown field '{}'", self.pointer),
      DiagnosticCode::EmptyStr => write!(f, "'{}' cannot be empty string", self.pointer),
      DiagnosticCode::EmptyMap => write!(f, "'{}' cannot be empty map", self.pointer),
      DiagnosticCode::UnknownAgent(agent) => write!(
        f,
        "agent profile '{}' set for '{}' is not defined in agents configuration section",
        agent, self.pointer
      ),
      DiagnosticCode::DuplicateStageName(stage_name) => {
        write!(
          f,
          "stage name '{}' set for '{}' is already defined",
          stage_name, self.pointer
        )
      },
    }
  }
}

impl Diagnostic {
  pub fn error(field: &str, code: DiagnosticCode) -> Self {
    Self {
      severity: DiagnosticSeverity::Error,
      code,
      pointer: field.to_string(),
    }
  }

  pub fn warning(field: &str, code: DiagnosticCode) -> Self {
    Self {
      severity: DiagnosticSeverity::Warning,
      code,
      pointer: field.to_string(),
    }
  }

  pub fn with_pointer<S: Into<String>>(mut self, pointer: S) -> Self {
    self.pointer = pointer.into();
    self
  }
}

/// Validators take the full schema so children can check cross-references
/// (e.g. an `IssueStage.agent` referencing the `agents` map).
pub trait Diagnose {
  fn diagnose(&self, workflow: &WorkflowSchema) -> Diagnostics;
}

#[cfg(test)]
mod tests {
  use super::*;

  #[test]
  fn diagnostics_extend_root_child_pointer_to_parent() {
    let mut child = Diagnostics::new();
    child.push(Diagnostic::error("", DiagnosticCode::EmptyMap));
    let mut diagnostics = Diagnostics::new();

    diagnostics.extends_with_pointer("agents", child);

    assert_eq!(diagnostics.errors.len(), 1);
    assert_eq!(diagnostics.errors[0].pointer, "agents");
    assert!(matches!(diagnostics.errors[0].code, DiagnosticCode::EmptyMap));
  }

  #[test]
  fn diagnostics_warn_unknown_fields_for_string_keys_only() {
    let mut fields = serde_yaml::Mapping::new();
    fields.insert(
      serde_yaml::Value::String("typo".to_string()),
      serde_yaml::Value::Bool(true),
    );
    fields.insert(serde_yaml::Value::Number(7.into()), serde_yaml::Value::Bool(true));
    let mut diagnostics = Diagnostics::new();

    diagnostics.warn_unknown_fields(&fields);

    assert_eq!(diagnostics.warnings.len(), 1);
    assert_eq!(diagnostics.warnings[0].pointer, "typo");
    assert!(matches!(diagnostics.warnings[0].code, DiagnosticCode::UnknownField));
  }
}
