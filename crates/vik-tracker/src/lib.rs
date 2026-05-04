mod normalize;
mod provider;
mod queries;
mod tracker;

#[cfg(test)]
mod tests;

pub use normalize::{dispatch_sort_key, normalize_issue};
pub use provider::{
    DEFAULT_GITHUB_ENDPOINT, DEFAULT_GITHUB_PAGE_SIZE, DEFAULT_LINEAR_ENDPOINT, DEFAULT_PAGE_SIZE,
    GitHubClient, GitHubClientConfig, GitHubIssueFilterConfig, LinearClient, LinearClientConfig,
    LinearIssueFilterConfig,
};
pub use queries::{
    ATTACHMENT_CREATE_MUTATION, CANDIDATE_QUERY, ISSUE_BY_IDENTIFIER_QUERY,
    ISSUE_STATES_BY_IDS_QUERY, ISSUES_BY_STATES_QUERY,
};
pub use tracker::TrackerClient;
