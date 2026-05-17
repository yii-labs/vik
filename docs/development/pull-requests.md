# Pull Requests

## Branch

- Use issue-scoped branches from `origin/main`.
- Sync before implementation and before handoff.

```sh
git fetch origin
git merge origin/main
```

## Commit

- Use English commit messages.
- Keep commits logical and reviewable.
- Include summary, rationale, and tests when commit body is useful.

```text
docs: add development testing rules

Summary:
- Add concise TDD and fixture rules.

Rationale:
- Keep agent test changes consistent.

Tests:
- cargo run --locked -- doctor --json ./workflow.yml
```

## PR

- Keep title, body, and labels current.
- Link the tracker issue.
- Summarize changed behavior or docs.
- List validation commands and results.
- Name risks or known gaps.
- Add the `vik` label.

## Review Loop

- Push latest branch.
- Check comments, reviews, CI, and mergeability.
- Address actionable items or reply with clear pushback.
- Rerun required checks.
- Update PR body and tracker state after scope changes.
- Move to human review only when CI is green and issue link is current.
