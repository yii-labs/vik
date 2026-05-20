# Stage `issues`

Issue: `{{ issue.id }}`: `{{ issue.title }}`
State: `{{ issue.state }}`
Workdir: `{{ issue.workdir }}`

## Start

Use __TRACKER_SKILL__ to read current tracker detail before stage work.

## Work

- Use __MATT_ISSUES_SKILL__ to split the PRD into implementation issues.
- Mark issues that are ready for automation with `ready`.
- Mark issues that need human decision with `HITL`.
- Keep tracker comments current.
- Move tracker state only after issue slicing is complete.

## Tracker Operations

Use __TRACKER_SKILL__ for comments, state moves, and PR links.

## Finish

- Record created issue links.
- Record which issues are `ready` and which need `HITL`.
