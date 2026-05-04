pub const SEARCH_ISSUES_PATH: &str = "/search/issues";

pub fn issue_path(owner: &str, repo: &str, number: u64) -> String {
    format!("/repos/{owner}/{repo}/issues/{number}")
}

pub fn issue_comments_path(owner: &str, repo: &str, number: u64) -> String {
    format!("/repos/{owner}/{repo}/issues/{number}/comments")
}

pub fn issue_comment_path(owner: &str, repo: &str, comment_id: u64) -> String {
    format!("/repos/{owner}/{repo}/issues/comments/{comment_id}")
}
