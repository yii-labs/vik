mod client;
mod queries;

#[cfg(test)]
mod tests;

pub use client::{
    DEFAULT_GITHUB_ENDPOINT, GitHubClient, GitHubClientConfig, GitHubIssueFilterConfig,
};
