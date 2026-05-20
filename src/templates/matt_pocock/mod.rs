pub(crate) mod prompts;

mod workflow;

use super::{SkillTemplate, StageTemplate, WorkflowTemplate};

const STAGES: &[StageTemplate] = &[
  StageTemplate::new("grill", "grill", prompts::GRILL),
  StageTemplate::new("prd", "prd", prompts::PRD),
  StageTemplate::new("issues", "issues", prompts::ISSUES),
  StageTemplate::new("work", "work", prompts::WORK),
  StageTemplate::new("review", "review", prompts::REVIEW),
  StageTemplate::new("merge", "merge", prompts::MERGE),
];

const SKILLS: &[SkillTemplate] = &[
  SkillTemplate::new(
    "Grill the plan",
    "grill-me",
    "__MATT_GRILL_SKILL__",
    include_str!("skills/grill-me.md"),
  ),
  SkillTemplate::new(
    "Grill with docs",
    "grill-with-docs",
    "__MATT_GRILL_WITH_DOCS_SKILL__",
    include_str!("skills/grill-with-docs.md"),
  ),
  SkillTemplate::new(
    "Write PRD",
    "to-prd",
    "__MATT_PRD_SKILL__",
    include_str!("skills/to-prd.md"),
  ),
  SkillTemplate::new(
    "Slice issues",
    "to-issues",
    "__MATT_ISSUES_SKILL__",
    include_str!("skills/to-issues.md"),
  ),
];

pub(crate) fn template() -> WorkflowTemplate {
  WorkflowTemplate::new(workflow::TEMPLATE, STAGES, SKILLS)
}
