# In-Memory Running State

Vik holds all running-issue and running-stage state in process memory. A crash or daemon restart loses the map entirely. On next start, Vik re-fetches issues through intake; if state still matches, stages re-dispatch naturally. There is no on-disk resume, no reattach to orphaned subprocesses, and no replay of partial runs.

We rejected persisting running state to disk because resume semantics are expensive (re-attach to live child processes across PID changes, reconcile partial hook state, replay session JSONL) and the reconciliation model ([ADR-0003](./0003-stateless-reconciliation-model.md)) already recovers correctly from a cold start. Durable state would duplicate the tracker as a source of truth.

Session JSONL files on disk are the only persisted runtime artifact; they are write-only history, not resumable state.
