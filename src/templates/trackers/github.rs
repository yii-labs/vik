use super::super::TrackerTemplate;

pub(crate) fn template() -> TrackerTemplate {
  TrackerTemplate::github_script(
    "github-issues-json",
    5,
    include_str!("github/issues-json.sh"),
    include_str!("github/read.md"),
    include_str!("github/operations.md"),
  )
}
