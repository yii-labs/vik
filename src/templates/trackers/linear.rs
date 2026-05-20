use super::super::{SkillTemplate, TrackerTemplate};

pub(crate) fn template() -> TrackerTemplate {
  TrackerTemplate::linear_script(
    "linear-issues-json",
    10,
    include_str!("linear/issues-json.sh"),
    "",
    SkillTemplate::new(
      "Linear Issues",
      "linear-issues",
      "__TRACKER_SKILL__",
      include_str!("linear/skill.md"),
    ),
  )
}
