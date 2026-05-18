//! Static setup templates used by `vik init`.

pub(crate) mod simple;
pub(crate) mod symphony;

mod trackers;

#[derive(Clone, Copy)]
pub(crate) struct WorkflowTemplate {
  workflow: &'static str,
  stages: &'static [StageTemplate],
}

impl WorkflowTemplate {
  pub(super) const fn new(workflow: &'static str, stages: &'static [StageTemplate]) -> Self {
    Self { workflow, stages }
  }

  pub(crate) fn stages(self) -> &'static [StageTemplate] {
    self.stages
  }

  pub(crate) fn render_workflow(self, tracker: TrackerTemplate) -> String {
    self
      .workflow
      .replace("__PULL_COMMAND__", &tracker.pull_command())
      .replace("__IDLE_SEC__", &tracker.idle_sec().to_string())
      .replace("__STAGES__", &self.render_stages())
  }

  pub(crate) fn render_prompt(self, stage: StageTemplate, tracker: TrackerTemplate) -> String {
    stage
      .prompt
      .replace("__TRACKER_READ__", tracker.read())
      .replace("__TRACKER_OPERATIONS__", tracker.operations())
  }

  fn render_stages(self) -> String {
    self
      .stages
      .iter()
      .map(|stage| {
        format!(
          "    {name}:\n      when:\n        state: {state}\n      agent: coder\n      prompt_file: ./.agents/prompts/{name}.md\n",
          name = stage.name,
          state = stage.state,
        )
      })
      .collect()
  }
}

#[derive(Clone, Copy)]
pub(crate) struct StageTemplate {
  pub(crate) name: &'static str,
  pub(crate) state: &'static str,
  prompt: &'static str,
}

impl StageTemplate {
  pub(super) const fn new(name: &'static str, state: &'static str, prompt: &'static str) -> Self {
    Self { name, state, prompt }
  }
}

#[derive(Clone, Copy)]
pub(crate) struct TrackerTemplate {
  script_name: &'static str,
  idle_sec: u64,
  script: TrackerScript,
  read: &'static str,
  operations: &'static str,
}

impl TrackerTemplate {
  pub(super) const fn static_script(
    script_name: &'static str,
    idle_sec: u64,
    script: &'static str,
    read: &'static str,
    operations: &'static str,
  ) -> Self {
    Self {
      script_name,
      idle_sec,
      script: TrackerScript::Static(script),
      read,
      operations,
    }
  }

  pub(super) const fn github_script(
    script_name: &'static str,
    idle_sec: u64,
    script: &'static str,
    read: &'static str,
    operations: &'static str,
  ) -> Self {
    Self {
      script_name,
      idle_sec,
      script: TrackerScript::Github(script),
      read,
      operations,
    }
  }

  pub(crate) fn script_name(self) -> &'static str {
    self.script_name
  }

  fn pull_command(self) -> String {
    format!("sh ./scripts/{}", self.script_name)
  }

  fn idle_sec(self) -> u64 {
    self.idle_sec
  }

  fn read(self) -> &'static str {
    self.read
  }

  fn operations(self) -> &'static str {
    self.operations
  }

  pub(crate) fn render_script(self, stages: &[StageTemplate]) -> String {
    match self.script {
      TrackerScript::Static(script) => script.to_string(),
      TrackerScript::Github(script) => render_github_script(script, stages),
    }
  }
}

#[derive(Clone, Copy)]
enum TrackerScript {
  Static(&'static str),
  Github(&'static str),
}

pub(crate) fn github_tracker() -> TrackerTemplate {
  trackers::github::template()
}

pub(crate) fn linear_tracker() -> TrackerTemplate {
  trackers::linear::template()
}

fn render_github_script(script: &'static str, stages: &[StageTemplate]) -> String {
  let labels = stages.iter().map(|stage| stage.state).collect::<Vec<_>>();
  let search_labels = labels.join(",");
  let jq_states = labels
    .iter()
    .map(|label| format!(". == \"{label}\""))
    .collect::<Vec<_>>()
    .join(" or ");

  script
    .replace("__STAGE_LABELS__", &search_labels)
    .replace("__JQ_STATES__", &jq_states)
}
