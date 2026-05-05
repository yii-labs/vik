# Linear Tracker

Use Linear when Vik should claim issues from one Linear project and route agent
tracker tools through the Linear API.

## Configuration

```yaml
tracker:
  kind: linear
  project_slug: "vik-08c9cf588aa7"
  active_states: [Todo, In Progress]
  terminal_states: [Done, Closed, Canceled, Duplicate]
  filter:
    assignees: [user-a]
    tags: [agent]
```

Fields:

- `endpoint`: defaults to `https://api.linear.app/graphql`.
- `api_key`: optional when `LINEAR_API_KEY` is set in the environment or
  `.env`.
- `project_slug`: Linear project slug Vik polls.
- `active_states`: Linear workflow states Vik may claim.
- `terminal_states`: Linear workflow states that stop tracking and may trigger
  workspace cleanup.
- `filter.assignees`: Linear user IDs, names, display names, or email
  addresses.
- `filter.tags`: Linear label names. Any listed label matches.

## Credentials

Set a Linear personal API key without printing or committing it:

```sh
export LINEAR_API_KEY=lin_api_xxx
vik check ./WORKFLOW.md
```

When `tracker.kind` is `linear`, Vik's app-server dynamic tools route issue
updates, comments, attachment uploads, and PR links through the configured
Linear credentials.
