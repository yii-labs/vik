# Stateless Reconciliation Model

Vik does not own issue state. Each intake cycle asks the issue prompt for
current issues, matches `issue.state` to `issue.stages.<stage>.when.state`, and
dispatches every matched stage that is not already running or reserved.

State transitions between stages happen through prompt-authored commands that
update the external tracker. Vik observes the new state on a later intake
cycle.

We rejected a Vik-owned state machine because routing ownership would split
between workflow config and runtime storage. Keeping the tracker as source of
truth makes Vik recoverable from crashes with no durable running-state store.

Current runtime guards:

- Deduplicate `(issue.id, stage.name)` while running or reserved.
- Limit active issue identifiers with `loop.max_issue_concurrency`.
- Preserve stage author order when multiple stages match one state.
