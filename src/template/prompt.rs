//! Two-phase prompt rendering: Jinja, then `exec(command)` substitution.

use std::time::Duration;

use regex::Regex;
use serde::Serialize;
use tokio::process::Command;

use crate::{
  shell::{CommandExecError, CommandExt},
  template::{TemplateError, jinja::JinjaRenderer},
};

pub struct PromptRenderer {
  inner: JinjaRenderer,
}

struct PromptCommand {
  start: usize,
  end: usize,
  command: String,
}

impl PromptRenderer {
  pub fn new() -> Self {
    Self {
      inner: JinjaRenderer::new(),
    }
  }

  pub async fn render<Context: Serialize>(&self, template: &str, context: Context) -> Result<String, TemplateError> {
    let jinja_rendered = self.inner.render(template, context)?;
    self.render_prompt(jinja_rendered).await
  }

  pub fn with_value<K: Into<String>, V: Serialize>(&mut self, key: K, value: V) -> &mut Self {
    self.inner.with_value(key, value);
    self
  }

  pub fn with_values<K: Into<String>, V: Serialize, It: Iterator<Item = (K, V)>>(&mut self, iter: It) -> &mut Self {
    self.inner.with_values(iter);
    self
  }

  pub fn with_envs<K: Into<String>, V: Into<String>, It: Iterator<Item = (K, V)>>(&mut self, iter: It) {
    self.inner.with_envs(iter);
  }

  pub fn with_env<K: Into<String>, V: Into<String>>(&mut self, key: K, value: V) {
    self.inner.with_env(key, value);
  }

  async fn render_prompt(&self, template: String) -> Result<String, TemplateError> {
    // Both ``!`exec(...)``` and ```exec(...)``` are supported. The
    // optional `!` prefix exists for parity with operator habits from
    // shell-based templating tools — there is no semantic difference.
    let re = Regex::new(r"!?`exec\(([^)]+)\)`").unwrap();

    let commands = re
      .captures_iter(&template)
      .filter_map(|captures| {
        let span = captures.get(0)?;
        let command = captures.get(1)?.as_str().trim().to_string();
        Some(PromptCommand {
          start: span.start(),
          end: span.end(),
          command,
        })
      })
      .collect::<Vec<_>>();

    if commands.is_empty() {
      return Ok(template);
    }

    // Run commands concurrently; substitution order is preserved by
    // splicing replacements back into the original byte ranges.
    let replacements =
      futures::future::try_join_all(commands.iter().map(|command| execute_prompt_command(&command.command))).await?;

    let mut rendered = String::with_capacity(template.len());
    let mut cursor = 0;
    for (command, replacement) in commands.iter().zip(replacements) {
      rendered.push_str(&template[cursor..command.start]);
      rendered.push_str(&replacement);
      cursor = command.end;
    }
    rendered.push_str(&template[cursor..]);

    Ok(rendered)
  }
}

async fn execute_prompt_command(command: &str) -> Result<String, TemplateError> {
  // 30s mirrors the hook timeout. A prompt command is supposed to be
  // a quick fact-fetch (e.g. `git rev-parse HEAD`); anything slower
  // belongs in a hook with proper observability.
  let output = shell_command(command)
    .timeout(Duration::from_secs(30))
    .output()
    .await
    .map_err(|err| TemplateError::PromptCommandFailed {
      command: command.to_string(),
      source: err,
    })?;

  if !output.status.success() {
    return Err(TemplateError::PromptCommandFailed {
      command: command.to_string(),
      source: CommandExecError::Spawn(std::io::Error::other(
        "Prompt template injection command exited with non-zero code.",
      )),
    });
  }

  let mut stdout = String::from_utf8_lossy(&output.stdout).into_owned();
  // Trim exactly one trailing newline so command output composes
  // naturally inside a sentence — `printf hello\n` should not splice a
  // line break into the surrounding template.
  if stdout.ends_with('\n') {
    stdout.pop();
  }

  Ok(stdout)
}

#[cfg(windows)]
fn shell_command(command: &str) -> Command {
  let mut shell = Command::new("cmd");
  shell.args(["/C", command]);
  shell
}

#[cfg(not(windows))]
fn shell_command(command: &str) -> Command {
  let mut shell = Command::new("sh");
  shell.args(["-c", command]);
  shell
}

#[cfg(test)]
mod tests {
  use serde_json::json;

  use super::*;

  #[tokio::test]
  async fn renders_jinja_before_executing_prompt_command() {
    let renderer = PromptRenderer::new();
    let rendered = renderer
      .render("hello !`exec(printf {{ name }})`", json!({ "name": "world" }))
      .await
      .expect("render");

    assert_eq!(rendered, "hello world");
  }

  #[tokio::test]
  async fn executes_all_prompt_commands_and_keeps_output_order() {
    let renderer = PromptRenderer::new();

    let rendered = renderer
      .render("!`exec(printf first)` then !`exec(printf second)`", json!({}))
      .await
      .expect("render");

    assert_eq!(rendered, "first then second");
  }

  #[tokio::test]
  async fn nonzero_prompt_command_returns_template_error() {
    let renderer = PromptRenderer::new();

    #[cfg(windows)]
    let fail_command = "echo bad 1>&2 & exit /b 7";
    #[cfg(not(windows))]
    let fail_command = "echo bad >&2; exit 7";

    let err = renderer
      .render(&format!("before !`exec({fail_command})` after"), json!({}))
      .await
      .expect_err("command must fail");

    assert!(matches!(err, TemplateError::PromptCommandFailed { .. }));
  }
}
