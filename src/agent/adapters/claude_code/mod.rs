//! Claude Code provider adapter.
//!
//! Command shape:
//!
//! ```text
//! claude --verbose --output-format stream-json --model <model> -p [profile.args...]
//! ```
//!
//! The prompt is piped on stdin (the position of `-p` is just where
//! the rest of `profile.args` are appended). Claude's NDJSON uses a
//! flat `type` discriminant; one `result` line carries both usage
//! totals and stream completion, so `map_line` may fan out into two
//! [`AgentEvent`]s for that single line.

mod event;

use crate::config::AgentProfileSchema;

use super::{AgentAdapter, AgentCommand, AgentEvent, AgentStdin, build_extra_args};

const CLAUDE_PROGRAM: &str = "claude";

#[derive(Debug, Clone)]
pub struct ClaudeCodeAdapter;

impl AgentAdapter for ClaudeCodeAdapter {
  fn build_command(&self, profile: &AgentProfileSchema, prompt: String) -> AgentCommand {
    let mut args: Vec<String> = vec![
      "--verbose".into(),
      "--output-format".into(),
      "stream-json".into(),
      "--model".into(),
      profile.model.clone(),
      "-p".into(),
    ];

    args.extend(build_extra_args(&profile.args));

    AgentCommand {
      program: CLAUDE_PROGRAM.into(),
      args,
      stdin: AgentStdin::Pipe(prompt),
    }
  }

  fn map_line(&self, line: &str) -> Result<Vec<AgentEvent>, serde_json::Error> {
    event::map_line(line)
  }
}

#[cfg(test)]
mod tests {
  use crate::config::AgentRuntime;

  use super::*;

  #[test]
  fn command_contains_expected_flags_and_closed_stdin() {
    let adapter = ClaudeCodeAdapter;
    let profile =
      AgentProfileSchema::new(AgentRuntime::ClaudeCode, "opus".into()).with_args(serde_yaml::Mapping::from_iter([
        (
          serde_yaml::Value::String("--permission-mode".into()),
          serde_yaml::Value::String("plan".into()),
        ),
        (
          serde_yaml::Value::String("--allowed-tools".into()),
          serde_yaml::Value::Sequence(vec![
            serde_yaml::Value::String("Edit".into()),
            serde_yaml::Value::String("Read".into()),
          ]),
        ),
        (
          serde_yaml::Value::String("--effort".into()),
          serde_yaml::Value::String("high".into()),
        ),
      ]));

    let cmd = adapter.build_command(&profile, "hello".into());
    assert_eq!(cmd.program, "claude");
    assert_eq!(
      cmd.args,
      vec![
        "--verbose",
        "--output-format",
        "stream-json",
        "--model",
        "opus",
        "-p",
        "--permission-mode",
        "plan",
        "--allowed-tools",
        "Edit,Read",
        "--effort",
        "high"
      ]
    );
    assert!(matches!(cmd.stdin, AgentStdin::Pipe(ref s) if s == "hello"));
  }
}
