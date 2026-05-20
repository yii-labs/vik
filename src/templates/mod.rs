//! Static setup templates used by `vik init`.

pub(crate) mod matt_pocock;
pub(crate) mod simple;
pub(crate) mod symphony;

mod trackers;

#[derive(Clone, Copy)]
pub(crate) struct WorkflowTemplate {
  workflow: &'static str,
  stages: &'static [StageTemplate],
  skills: &'static [SkillTemplate],
}

impl WorkflowTemplate {
  pub(super) const fn new(
    workflow: &'static str,
    stages: &'static [StageTemplate],
    skills: &'static [SkillTemplate],
  ) -> Self {
    Self {
      workflow,
      stages,
      skills,
    }
  }

  pub(crate) fn stages(self) -> &'static [StageTemplate] {
    self.stages
  }

  pub(crate) fn skills(self) -> &'static [SkillTemplate] {
    self.skills
  }

  pub(crate) fn render_workflow(self, tracker: TrackerTemplate) -> String {
    self
      .workflow
      .replace("__PULL_COMMAND__", &tracker.pull_command())
      .replace("__IDLE_SEC__", &tracker.idle_sec().to_string())
      .replace("__STAGES__", &self.render_stages())
  }

  pub(crate) fn render_prompt(
    self,
    stage: StageTemplate,
    tracker: TrackerTemplate,
    skills: &[SkillNameBinding],
  ) -> String {
    let mut prompt = stage.prompt.replace("__TRACKER_CONTEXT__", tracker.prompt_context());
    for skill in skills {
      prompt = prompt.replace(skill.placeholder, &format!("${}", skill.name));
    }

    prompt
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
pub(crate) struct SkillTemplate {
  pub(crate) display_name: &'static str,
  pub(crate) default_name: &'static str,
  pub(crate) placeholder: &'static str,
  contents: &'static str,
}

impl SkillTemplate {
  pub(super) const fn new(
    display_name: &'static str,
    default_name: &'static str,
    placeholder: &'static str,
    contents: &'static str,
  ) -> Self {
    Self {
      display_name,
      default_name,
      placeholder,
      contents,
    }
  }

  pub(crate) fn render_contents(self, name: &str) -> String {
    self
      .contents
      .replace(&format!("name: {}", self.default_name), &format!("name: {name}"))
  }
}

pub(crate) struct SkillNameBinding {
  pub(crate) placeholder: &'static str,
  pub(crate) name: String,
}

#[derive(Clone, Copy)]
pub(crate) struct TrackerTemplate {
  script_name: &'static str,
  idle_sec: u64,
  script: TrackerScript,
  prompt_context: &'static str,
  skill: SkillTemplate,
}

impl TrackerTemplate {
  pub(super) const fn linear_script(
    script_name: &'static str,
    idle_sec: u64,
    script: &'static str,
    prompt_context: &'static str,
    skill: SkillTemplate,
  ) -> Self {
    Self {
      script_name,
      idle_sec,
      script: TrackerScript::Linear(script),
      prompt_context,
      skill,
    }
  }

  pub(super) const fn github_script(
    script_name: &'static str,
    idle_sec: u64,
    script: &'static str,
    prompt_context: &'static str,
    skill: SkillTemplate,
  ) -> Self {
    Self {
      script_name,
      idle_sec,
      script: TrackerScript::Github(script),
      prompt_context,
      skill,
    }
  }

  pub(super) const fn github_projects_script(
    script_name: &'static str,
    idle_sec: u64,
    script: &'static str,
    prompt_context: &'static str,
    skill: SkillTemplate,
  ) -> Self {
    Self {
      script_name,
      idle_sec,
      script: TrackerScript::GithubProjects(script),
      prompt_context,
      skill,
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

  fn prompt_context(self) -> &'static str {
    self.prompt_context
  }

  pub(crate) fn skill(self) -> SkillTemplate {
    self.skill
  }

  pub(crate) fn render_script(self, stages: &[StageTemplate]) -> String {
    match self.script {
      TrackerScript::Github(script) => render_github_script(script, stages),
      TrackerScript::GithubProjects(script) => render_github_script(script, stages),
      TrackerScript::Linear(script) => render_linear_script(script, stages),
    }
  }
}

#[derive(Clone, Copy)]
enum TrackerScript {
  Github(&'static str),
  GithubProjects(&'static str),
  Linear(&'static str),
}

pub(crate) fn github_tracker() -> TrackerTemplate {
  trackers::github::template()
}

pub(crate) fn github_projects_tracker() -> TrackerTemplate {
  trackers::github_projects::template()
}

pub(crate) fn linear_tracker() -> TrackerTemplate {
  trackers::linear::template()
}

fn render_github_script(script: &'static str, stages: &[StageTemplate]) -> String {
  let labels = stages.iter().map(|stage| stage.state).collect::<Vec<_>>();
  let jq_states = labels
    .iter()
    .map(|label| format!(". == \"{label}\""))
    .collect::<Vec<_>>()
    .join(" or ");

  script.replace("__JQ_STATES__", &jq_states)
}

fn render_linear_script(script: &'static str, stages: &[StageTemplate]) -> String {
  let states = stages.iter().map(|stage| stage.state).collect::<Vec<_>>();
  let states_json = serde_json::to_string(&states).expect("stage names serialize");
  script.replace("__STATE_NAMES_JSON__", &states_json)
}
