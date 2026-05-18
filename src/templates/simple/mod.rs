pub(crate) mod prompts;

mod workflow;

use super::{StageTemplate, WorkflowTemplate};

const STAGES: &[StageTemplate] = &[
  StageTemplate::new("work", "work", prompts::WORK),
  StageTemplate::new("review", "review", prompts::REVIEW),
];

pub(crate) fn template() -> WorkflowTemplate {
  WorkflowTemplate::new("Simple", workflow::TEMPLATE, STAGES)
}
