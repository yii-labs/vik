mod client;
mod normalize;
mod queries;

#[cfg(test)]
mod tests;

pub use client::{
    DEFAULT_LINEAR_ENDPOINT, DEFAULT_PAGE_SIZE, LinearClient, LinearClientConfig,
    LinearIssueFilterConfig,
};
pub use normalize::{dispatch_sort_key, normalize_issue};
pub use queries::{
    ATTACHMENT_CREATE_MUTATION, CANDIDATE_QUERY, ISSUE_BY_IDENTIFIER_QUERY,
    ISSUE_STATES_BY_IDS_QUERY, ISSUES_BY_STATES_QUERY,
};
