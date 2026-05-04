use minijinja::{Environment, UndefinedBehavior, context};
use serde_json::json;
use vik_core::{Issue, WorkflowDefinition};

use crate::WorkflowError;

const DEFAULT_PROMPT: &str = "You are working on an issue.";

pub fn render_prompt(
    definition: &WorkflowDefinition,
    issue: &Issue,
    attempt: Option<u32>,
) -> Result<String, WorkflowError> {
    let template = if definition.prompt_template.is_empty() {
        DEFAULT_PROMPT
    } else {
        &definition.prompt_template
    };
    let mut env = Environment::new();
    env.set_undefined_behavior(UndefinedBehavior::Strict);
    env.add_template("workflow", template)
        .map_err(|err| WorkflowError::TemplateParseError(err.to_string()))?;
    let tmpl = env
        .get_template("workflow")
        .map_err(|err| WorkflowError::TemplateParseError(err.to_string()))?;
    tmpl.render(context! {
        issue => serde_json::to_value(issue).unwrap_or_else(|_| json!({})),
        attempt => attempt,
    })
    .map_err(|err| WorkflowError::TemplateRenderError(err.to_string()))
}
