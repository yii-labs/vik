# Stage `work`

Issue: `{{ issue.id }}`: `{{ issue.title }}`
State: `{{ issue.state }}`
Workdir: `{{ issue.workdir }}`
__TRACKER_CONTEXT__

## Start

Use __TRACKER_SKILL__ to read current tracker detail before stage work.

## Work

- Implement only scoped issue work.
- Keep tracker comments current.
- Move tracker state only after validation passes.

## Tracker Operations

Use __TRACKER_SKILL__ for comments, state moves, and PR links.

## Finish

- Record changed files.
- Record validation commands and results.
