//! In-memory registry of reserved and running stages.
//!
//! Owned exclusively by the orchestrator main loop — background tasks
//! report through events instead of mutating directly. Reservation lets
//! the main loop claim a stage key before spawning setup, which is what
//! prevents duplicate intake events from racing into a second launch.

use std::collections::{HashMap, HashSet};

use crate::context::{IssueStage, IssueStageKey};
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

  pub(super) fn cancel_all(&self) {
    for stage in self.stages.values() {
      if let Some(session) = &stage.session {
        session.cancel();
      }
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
