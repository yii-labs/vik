use super::super::TrackerTemplate;

pub(crate) fn template() -> TrackerTemplate {
  TrackerTemplate::github_projects_script(
    "github-project-items-json",
    5,
    include_str!("github_projects/items-json.sh"),
    include_str!("github_projects/read.md"),
    include_str!("github_projects/operations.md"),
  )
}
