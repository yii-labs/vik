# GitHub Tracker

Use GitHub when Vik should claim GitHub issues from one repository.

## Configuration

```yaml
tracker:
  kind: github
  repository: yii-labs/vik
  active_states: [Todo, In Progress]
  terminal_states: [Done, Closed, Duplicate]
  filter:
    assignees: [forehalo]
    tags: [agent]
```

Fields:

- `endpoint`: defaults to `https://api.github.com`.
- `api_key`: optional when `GH_TOKEN` or `GITHUB_TOKEN` is set in the
  environment or `.env`.
- `repository`: GitHub repository in `owner/name` form. HTTPS and SSH GitHub
  clone URLs are accepted and normalized.
- `active_states`: states Vik may claim. `open` maps to the GitHub issue state;
  other values are read from labels on open issues.
- `terminal_states`: states that stop tracking and may trigger workspace
  cleanup. `done`, `close`, and `closed` map to the GitHub closed issue state;
  other values are read from labels.
- `filter.assignees`: GitHub login names. Any listed assignee matches.
- `filter.tags`: GitHub label names. Any listed label matches.

## Credentials

Set a token with repository issue access:

```sh
export GH_TOKEN=github_pat_xxx
vik check ./WORKFLOW.md
```

For private repositories, grant at least repository metadata read access and
issues read/write access. GitHub PR linking updates pull request bodies, so the
token also needs pull request write access for repositories that will receive
linked PRs. Agents still need normal GitHub access for branch, push, PR, review,
and check operations.

## Behavior

Vik uses GitHub issue search with `is:issue`, so pull requests are not claimed as
tracker candidates.

GitHub issue attachments cannot be uploaded through the GitHub Issues API.
Vik returns an unsupported-operation error for tracker attachment upload with
`tracker.kind: github`; attach files to PRs or comments through another
workflow when needed.

PR linking for GitHub issues updates the pull request body with a GitHub closing
keyword such as `Closes owner/repo#123`. GitHub interprets closing keywords only
for pull requests that target the repository default branch; for other PR bases,
the body still records the issue relationship but GitHub will not auto-close the
issue on merge.
