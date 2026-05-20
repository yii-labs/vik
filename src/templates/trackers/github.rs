use super::super::{SkillTemplate, TrackerTemplate};

pub(crate) fn template() -> TrackerTemplate {
  TrackerTemplate::github_script(
    "github-issues-json",
    5,
    include_str!("github/issues-json.sh"),
    "",
    SkillTemplate::new(
      "GitHub Issues",
      "github-issues",
      "__TRACKER_SKILL__",
      include_str!("github/skill.md"),
    ),
  )
}
