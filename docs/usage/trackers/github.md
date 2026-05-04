# GitHub Tracker

Use the GitHub tracker when Vik should claim GitHub issues from one repository.
Pull requests returned by the GitHub issues API are ignored.
Vik renders GitHub issue identifiers as `GH-<number>` so observation routes
such as `/api/v1/GH-42` stay path-safe.

## Configuration

```yaml
tracker:
  kind: github
  repository: yii-labs/vik
  active_states:
    - open
  terminal_states:
    - closed
```

Fields:

- `endpoint`: defaults to `https://api.github.com`.
- `api_key`: optional when `GH_TOKEN` or `GITHUB_TOKEN` is set.
- `repository`: required GitHub repository in `owner/name` form.
- `active_states`: GitHub issue states Vik may claim. Use `open` or `closed`.
- `terminal_states`: GitHub issue states that stop tracking and may trigger
  cleanup. Use `closed` for normal operation.
- `filter.assignees`: GitHub usernames.
- `filter.tags`: GitHub label names.

`GH_TOKEN` and `GITHUB_TOKEN` are loaded from `.env` before dispatch validation.
Do not commit real keys.

Limit delegation to open issues assigned to specific users and labeled for
agent work:

```yaml
tracker:
  kind: github
  repository: yii-labs/vik
  active_states: [open]
  terminal_states: [closed]
  filter:
    assignees: [forehalo]
    tags: [agent, codex]
```

## Token Scope

Use a fine-grained GitHub token limited to the configured repository when
possible. Grant at least:

- `metadata: read`
- `issues: read`

Agents still need normal repository and pull request credentials for branch,
push, PR, review, and check operations described in [Get Started](../get-started.md).

## Validate

For config-only validation, a placeholder token is enough because `vik check`
validates config shape and does not call GitHub:

```sh
GH_TOKEN=ci-placeholder vik check ./WORKFLOW.md
```
