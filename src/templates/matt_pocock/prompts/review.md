# Stage `review`

Issue: `{{ issue.id }}`: `{{ issue.title }}`
State: `{{ issue.state }}`
Workdir: `{{ issue.workdir }}`

## Start

Use __TRACKER_SKILL__ to read current tracker detail before stage work.

## Work

- Review implementation for bugs, regressions, and missing tests.
- Keep tracker comments current.
- Move tracker state only after review feedback is resolved or recorded.

## Tracker Operations

Use __TRACKER_SKILL__ for comments, state moves, and PR links.

## Finish

- Record review outcome.
- Record validation commands and results.
