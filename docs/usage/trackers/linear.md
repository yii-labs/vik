# Linear Tracker

Use the Linear tracker when Vik should claim Linear issues from one Linear
project.

## Configuration

```yaml
tracker:
  kind: linear
  project_slug: "vik-08c9cf588aa7"
  active_states:
    - Todo
    - In Progress
  terminal_states:
    - Closed
    - Cancelled
    - Canceled
    - Duplicate
    - Done
```

Fields:

- `endpoint`: defaults to `https://api.linear.app/graphql`.
- `api_key`: optional when `LINEAR_API_KEY` is set.
- `project_slug`: Linear project slug Vik polls.
- `active_states`: Linear workflow state names Vik may claim.
- `terminal_states`: Linear workflow state names that stop tracking and may
  trigger cleanup.
- `filter.assignees`: Linear user IDs, names, display names, or email
  addresses.
- `filter.tags`: Linear label names.

`LINEAR_API_KEY` is loaded from `.env` before dispatch validation. Do not commit
real keys.

Limit delegation to issues assigned to specific users and tagged with specific
Linear labels:

```yaml
tracker:
  kind: linear
  project_slug: "vik-08c9cf588aa7"
  filter:
    assignees: [user-a, user-b]
    tags: [agent, codex]
```

## Validate

For config-only validation, a placeholder token is enough because `vik check`
does not call Linear:

```sh
LINEAR_API_KEY=ci-placeholder vik check ./WORKFLOW.md
```
