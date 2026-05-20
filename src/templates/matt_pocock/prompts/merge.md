# Stage `merge`

Issue: `{{ issue.id }}`: `{{ issue.title }}`
State: `{{ issue.state }}`
Workdir: `{{ issue.workdir }}`

## Start

Use __TRACKER_SKILL__ to read current tracker detail before stage work.

## Work

- Confirm latest validation and review state.
- Link the pull request to the issue before merge.
- Move tracker state only after merge or handoff is complete.

## Tracker Operations

Use __TRACKER_SKILL__ for comments, state moves, and PR links.

## Finish

- Record PR URL.
- Record merge or blocker status.
