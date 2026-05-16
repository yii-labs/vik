//! In-memory registry of reserved and running stages.
//!
//! Owned exclusively by the orchestrator main loop — background tasks
//! report through events instead of mutating directly. Reservation lets
//! the main loop claim a stage key before spawning setup, which is what
//! prevents duplicate intake events from racing into a second launch.

use std::collections::{HashMap, HashSet};

use crate::context::{IssueStage, IssueStageKey};
use crate::logging::stage_span;
use crate::session::{Session, SessionSnapshot};

pub(super) struct RunningMap {
  max_issue_concurrency: usize,
  stages: HashMap<IssueStageKey, RunningStage>,
}

impl RunningMap {
  pub(super) fn new(max_issue_concurrency: usize) -> Self {
    Self {
      max_issue_concurrency,
      stages: HashMap::new(),
    }
  }

  pub(super) fn is_empty(&self) -> bool {
    self.stages.is_empty()
  }

  pub(super) fn contains_key(&self, key: &IssueStageKey) -> bool {
    self.stages.contains_key(key)
  }

  /// A new stage for an already-running issue is always allowed. The
  /// concurrency cap counts distinct issue ids, not stages — an
  /// issue with three matching stages must not be blocked from its own
  /// stages by some other busy issue.
  pub(super) fn can_accept_issue(&self, issue_id: &str) -> bool {
    self.contains_issue(issue_id) || self.running_issue_count() < self.max_issue_concurrency
  }

  pub(super) fn reserve(&mut self, issue_stage: IssueStage) -> bool {
    let key = issue_stage.key();
    if self.contains_key(&key) || !self.can_accept_issue(&key.issue_id) {
      return false;
    }

    self.stages.insert(key, RunningStage::reserved(issue_stage));
    true
  }

  pub(super) fn start(&mut self, issue_stage: IssueStage, session: Session) {
    let key = issue_stage.key();
    let snapshot = session.snapshot();
    self
      .stages
      .entry(key)
      .and_modify(|stage| stage.start(session.clone(), snapshot.clone()))
      .or_insert_with(|| RunningStage::started(issue_stage, session, snapshot));
  }

  pub(super) fn update(&mut self, key: &IssueStageKey, snapshot: SessionSnapshot) {
    if let Some(stage) = self.stages.get_mut(key) {
      stage.snapshot = Some(snapshot);
    }
  }

  pub(super) fn finish(&mut self, key: &IssueStageKey, snapshot: SessionSnapshot) -> Option<RunningStage> {
    self.update(key, snapshot);
    self.stages.remove(key)
  }

  /// Removal path for setup/spawn failures. Distinct from `finish`
  /// because there is no terminal snapshot to record.
  pub(super) fn fail(&mut self, key: &IssueStageKey) -> Option<RunningStage> {
    self.stages.remove(key)
  }

  /// Each cancel runs inside the stage's own span so the lifecycle log
  /// emitted from `Session::cancel` carries the same `phase` / `issue_id`
  /// / `stage_name` / `agent_profile` fields as the rest of the stage
  /// run. Without this, hard-shutdown cancellations would emit
  /// uncorrelated logs because `cancel_all` is called from the main
  /// loop, outside any stage span.
  pub(super) fn cancel_all(&self) {
    for stage in self.stages.values() {
      let Some(session) = &stage.session else {
        continue;
      };
      let _entered = stage_span(
        &stage.issue_stage.issue().id,
        stage.issue_stage.stage_name(),
        &stage.issue_stage.stage().agent,
      )
      .entered();
      session.cancel();
    }
  }

  fn contains_issue(&self, issue_id: &str) -> bool {
    self.stages.values().any(|stage| stage.issue_stage.issue().id == issue_id)
  }

  /// Distinct issue ids, not stage entries — see `can_accept_issue`.
  fn running_issue_count(&self) -> usize {
    self
      .stages
      .values()
      .map(|stage| stage.issue_stage.issue().id.as_str())
      .collect::<HashSet<_>>()
      .len()
  }
}

/// `session = None` is the reservation marker — a key is claimed but no
/// session has spawned yet.
pub(super) struct RunningStage {
  pub(super) issue_stage: IssueStage,
  pub(super) session: Option<Session>,
  pub(super) snapshot: Option<SessionSnapshot>,
}

impl RunningStage {
  fn reserved(issue_stage: IssueStage) -> Self {
    Self {
      issue_stage,
      session: None,
      snapshot: None,
    }
  }

  fn started(issue_stage: IssueStage, session: Session, snapshot: SessionSnapshot) -> Self {
    Self {
      issue_stage,
      session: Some(session),
      snapshot: Some(snapshot),
    }
  }

  fn start(&mut self, session: Session, snapshot: SessionSnapshot) {
    self.session = Some(session);
    self.snapshot = Some(snapshot);
  }
}

#[cfg(test)]
mod tests {
  use std::sync::Arc;

  use super::*;
  use crate::context::{Issue, IssueRun};
  use crate::session::SessionState;
  use crate::workflow::Workflow;

  #[test]
  fn reserve_claims_stage_key_until_fail_releases_it() {
    let issue_stage = issue_stage("ABC-1", "plan", "todo");
    let key = issue_stage.key();
    let mut running = RunningMap::new(10);

    assert!(running.reserve(issue_stage.clone()));
    assert!(running.contains_key(&key));
    assert!(!running.reserve(issue_stage.clone()));

    let failed = running.fail(&key).expect("reserved stage removed");

    assert_eq!(failed.issue_stage.key(), key);
    assert!(failed.session.is_none());
    assert!(failed.snapshot.is_none());
    assert!(!running.contains_key(&key));
    assert!(running.reserve(issue_stage));
  }

  #[test]
  fn concurrency_counts_distinct_issues_and_allows_more_stages_for_same_issue() {
    let mut issue_stages = issue_stages("ABC-1", "todo", &["plan", "implement"]).into_iter();
    let plan = issue_stages.next().expect("plan stage");
    let implement = issue_stages.next().expect("implement stage");
    let other_issue = issue_stage("ABC-2", "plan", "todo");
    let other_key = other_issue.key();
    let mut running = RunningMap::new(1);

    assert!(running.reserve(plan));
    assert!(running.can_accept_issue("ABC-1"));
    assert!(running.reserve(implement));
    assert!(!running.can_accept_issue("ABC-2"));
    assert!(!running.reserve(other_issue));
    assert!(!running.contains_key(&other_key));
  }

  #[test]
  fn update_and_finish_keep_latest_snapshot_and_remove_stage() {
    let issue_stage = issue_stage("ABC-1", "plan", "todo");
    let key = issue_stage.key();
    let mut running = RunningMap::new(10);

    assert!(running.reserve(issue_stage));

    running.update(&key, snapshot(SessionState::Running, "in progress"));

    let reserved = running.stages.get(&key).expect("reserved stage remains");
    assert_eq!(
      reserved.snapshot.as_ref().expect("snapshot recorded").last_message.as_deref(),
      Some("in progress")
    );

    let finished = running
      .finish(&key, snapshot(SessionState::Completed, "done"))
      .expect("finished stage removed");

    assert_eq!(
      finished
        .snapshot
        .as_ref()
        .expect("terminal snapshot recorded")
        .last_message
        .as_deref(),
      Some("done")
    );
    assert!(finished.snapshot.expect("terminal snapshot recorded").state.is_terminated());
    assert!(!running.contains_key(&key));
    assert!(running.finish(&key, snapshot(SessionState::Completed, "ignored")).is_none());
    assert!(running.fail(&key).is_none());
  }

  fn issue_stage(issue_id: &str, stage_name: &str, state: &str) -> IssueStage {
    issue_stages(issue_id, state, &[stage_name])
      .into_iter()
      .next()
      .expect("stage fixture exists")
  }

  fn issue_stages(issue_id: &str, state: &str, stage_names: &[&str]) -> Vec<IssueStage> {
    let mut builder = Workflow::builder();
    for stage_name in stage_names {
      builder = builder.add_stage(*stage_name, state, format!("./{stage_name}.md"));
    }

    let workflow = Arc::new(builder.build());
    let issue_run = Arc::new(IssueRun::new(Arc::clone(&workflow), issue(issue_id, state)));

    IssueRun::matching_stages(issue_run)
  }

  fn issue(id: &str, state: &str) -> Issue {
    Issue {
      id: id.to_string(),
      title: "title".to_string(),
      description: String::new(),
      state: state.to_string(),
      extra_payload: serde_yaml::Mapping::new(),
    }
  }

  fn snapshot(state: SessionState, last_message: &str) -> SessionSnapshot {
    SessionSnapshot {
      state,
      last_message: Some(last_message.to_string()),
      ..Default::default()
    }
  }
}
