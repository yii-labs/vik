# Pull Requests

## Branches

Use issue-scoped branches:

```sh
git switch -c vik-16-docs origin/main
```

Sync with `origin/main` before implementation and before handoff:

```sh
git fetch origin
git merge origin/main
```

## Commits

Use English commit messages. Keep commits logical and reviewable.

Message shape:

```text
docs: add Vik usage and development guides

Summary:
- Add operator docs for startup, service, config, and observation.
- Add development docs and agent index.

Rationale:
- Keep README small and make setup steps executable by agents.

Tests:
- cargo test --all-features
```

## PR Body

Include:

- issue identifier
- summary of changed behavior or docs
- validation commands and results
- risks or known gaps

Add the `vik` label.

## Review Loop

Before moving the tracker issue to human review:

1. Push latest branch.
2. Check PR comments, inline comments, and review summaries.
3. Address every actionable item or reply with clear pushback.
4. Rerun required checks.
5. Confirm CI is green.
6. Confirm PR is linked to the tracker issue.

Do not leave stale workpad or PR body text after scope changes.
