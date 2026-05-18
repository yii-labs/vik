pub(crate) mod prompts;

mod workflow;

use super::{StageTemplate, WorkflowTemplate};

const STAGES: &[StageTemplate] = &[
  StageTemplate::new("plan", "plan", prompts::PLAN),
  StageTemplate::new("rework", "rework", prompts::REWORK),
  StageTemplate::new("work", "work", prompts::WORK),
  StageTemplate::new("review", "review", prompts::REVIEW),
  StageTemplate::new("merge", "merge", prompts::MERGE),
];

pub(crate) fn template() -> WorkflowTemplate {
  WorkflowTemplate::new(workflow::TEMPLATE, STAGES)
}
