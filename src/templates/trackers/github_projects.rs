use super::super::{SkillTemplate, TrackerTemplate};

pub(crate) fn template() -> TrackerTemplate {
  TrackerTemplate::github_projects_script(
    "github-project-items-json",
    5,
    include_str!("github_projects/items-json.sh"),
    SkillTemplate::new(
      "GitHub Projects",
      "github-projects",
      "__TRACKER_SKILL__",
      include_str!("github_projects/skill.md"),
    ),
  )
}
