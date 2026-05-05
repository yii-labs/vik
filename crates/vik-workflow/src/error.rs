use thiserror::Error;

#[derive(Debug, Error)]
pub enum WorkflowError {
    #[error("missing_workflow_file")]
    MissingWorkflowFile,
    #[error("workflow_parse_error: {0}")]
    WorkflowParseError(String),
    #[error("workflow_front_matter_not_a_map")]
    WorkflowFrontMatterNotAMap,
    #[error("template_parse_error: {0}")]
    TemplateParseError(String),
    #[error("template_render_error: {0}")]
    TemplateRenderError(String),
    #[error("unsupported_tracker_kind")]
    UnsupportedTrackerKind,
    #[error("missing_tracker_api_key")]
    MissingTrackerApiKey,
    #[error("missing_tracker_project_slug")]
    MissingTrackerProjectSlug,
    #[error("missing_tracker_repository")]
    MissingTrackerRepository,
    #[error("invalid_tracker_repository: {0}")]
    InvalidTrackerRepository(String),
    #[error("invalid_config: {0}")]
    InvalidConfig(String),
}

impl From<vik_tracker::TrackerConfigError> for WorkflowError {
    fn from(value: vik_tracker::TrackerConfigError) -> Self {
        match value {
            vik_tracker::TrackerConfigError::UnsupportedTrackerKind => Self::UnsupportedTrackerKind,
            vik_tracker::TrackerConfigError::MissingApiKey => Self::MissingTrackerApiKey,
            vik_tracker::TrackerConfigError::MissingProjectSlug => Self::MissingTrackerProjectSlug,
            vik_tracker::TrackerConfigError::MissingRepository => Self::MissingTrackerRepository,
            vik_tracker::TrackerConfigError::InvalidRepository(repository) => {
                Self::InvalidTrackerRepository(repository)
            }
        }
    }
}
