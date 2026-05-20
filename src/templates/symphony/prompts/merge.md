# Stage `merge`

Issue: `{{ issue.id }}`: `{{ issue.title }}`
State: `{{ issue.state }}`
Workdir: `{{ issue.workdir }}`

## Start

Use __TRACKER_SKILL__ to read current tracker detail before stage work.

## Work

- Use __SYMPHONY_SKILL__ for Symphony stage rules.
- Do only the work for this stage.
- Keep tracker comments current.
- Move tracker state only after this stage is complete.

## Tracker Operations

Use __TRACKER_SKILL__ for comments, state moves, and PR links.

## Finish

- Record what changed.
- Record validation commands and results.
- Move state to the next workflow state when complete.
