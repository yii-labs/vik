mod client;
mod github;
mod normalize;
mod queries;
mod tracker;

#[cfg(test)]
mod tests;

pub use client::{
    DEFAULT_LINEAR_ENDPOINT, DEFAULT_PAGE_SIZE, LinearClient, LinearClientConfig,
    LinearIssueFilterConfig,
};
pub use github::{
    DEFAULT_GITHUB_ENDPOINT, DEFAULT_GITHUB_PAGE_SIZE, GitHubClient, GitHubClientConfig,
    GitHubIssueFilterConfig,
};
pub use normalize::{dispatch_sort_key, normalize_issue};
pub use queries::{
    ATTACHMENT_CREATE_MUTATION, CANDIDATE_QUERY, ISSUE_BY_IDENTIFIER_QUERY,
    ISSUE_STATES_BY_IDS_QUERY, ISSUES_BY_STATES_QUERY,
};
pub use tracker::TrackerClient;
