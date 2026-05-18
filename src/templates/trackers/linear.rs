use super::super::TrackerTemplate;

pub(crate) fn template() -> TrackerTemplate {
  TrackerTemplate::static_script(
    "linear-issues-json",
    10,
    include_str!("linear/issues-json.sh"),
    include_str!("linear/read.md"),
    include_str!("linear/operations.md"),
  )
}
