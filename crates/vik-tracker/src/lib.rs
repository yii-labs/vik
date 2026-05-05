pub mod providers;

pub use vik_core::{IssueAttachment, IssueComment, IssueTracker, IssueUpdate};

pub use providers::{
    CommonTrackerConfig, TrackerClient, TrackerConfig, TrackerConfigError, TrackerFilterConfig,
    TrackerKind,
    github::{
        DEFAULT_GITHUB_ENDPOINT, GitHubClient, GitHubClientConfig, GitHubIssueFilterConfig,
        GitHubTrackerConfig,
    },
    linear::{
        DEFAULT_LINEAR_ENDPOINT, DEFAULT_PAGE_SIZE, LinearClient, LinearClientConfig,
        LinearIssueFilterConfig, LinearTrackerConfig,
    },
};
