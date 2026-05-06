# Feishu Tracker

Use Feishu when Vik should claim issues from one Feishu Base table and route
agent tracker tools through `lark-cli`.

## Configuration

```yaml
tracker:
  kind: feishu
  base_token: P5wZbJ2OiaETjdseIUJczdXqnle
  table_id: tblUqPdAnvAcPY6T
  view_id: vewpBV8AK0
  active_states: [Todo, In Progress]
  terminal_states: [Done, Closed, Canceled, Duplicate]
  filter:
    tags: [agent]
```

Fields:

- `base_token`: Feishu Base token.
- `table_id`: table ID. Vik only reads and writes this table.
- `view_id`: optional view ID or name used for list reads. Configure this for
  large tables so candidate and state scans read only the intended view.
- `cli_path`: optional path to `lark-cli`. Default: `lark-cli`.
- `identity`: optional `lark-cli --as` identity type, either `user` or `bot`.
  Default: `user`.
- `active_states`: Base state values Vik may claim.
- `terminal_states`: Base state values that stop tracking and may trigger
  workspace cleanup.
- `filter.tags`: optional labels matched against the configured labels field.
  `filter.assignees` is ignored for Feishu.

## Table Fields

The default field mapping matches a simple issue table:

```yaml
tracker:
  fieldsMap:
    title: Title
    state: State
    labels: Labels
    comments: Workpad
    pr_links: PR Links
```

Required fields:

- `Title`: issue title.
- `State`: single-select state field.
- `Labels`: multi-select label storage used by `update_issue`.
- `Workpad`: text field containing the single Vik-managed workpad comment as
  plain text.
- `PR Links`: text field containing Vik-managed pull request link.

Configure optional `fieldsMap.description` to include a description field in
prompt context. Vik always uses the Feishu record ID as the provider issue ID
and display identifier. Candidate selection relies on `tracker.view_id`, state,
and label filters.

## Credentials

Install and authenticate `lark-cli` before starting Vik:

```sh
lark-cli auth status --verify
```

Vik does not read a Feishu API key from `WORKFLOW.md` or `.env`; the CLI profile
provides the token used for Base operations.

## Behavior

Vik uses `lark-cli base` commands for record reads, record updates, comments, and PR links. Comment operations update the configured Workpad text field directly.

Feishu attachment upload is not implemented unless a future workflow configures a Base attachment field for that purpose.
