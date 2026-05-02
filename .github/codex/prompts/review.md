# Codex Pull Request Review

You are reviewing a pull request for Vik.

Use the checked-out repository and Git history to inspect the PR diff. Prefer
these commands when the referenced SHAs are available:

```sh
git diff --stat "$PR_BASE_SHA...$PR_HEAD_SHA"
git diff --find-renames "$PR_BASE_SHA...$PR_HEAD_SHA"
```

Focus on issues that should block merge:

- Correctness bugs.
- Regressions against existing behavior.
- Missing validation for changed behavior.
- Security or secret-handling risks.
- Workflow or CI errors that will fail on GitHub Actions.

Do not rewrite files. Do not post broad style advice. Do not repeat the PR
summary unless it supports a finding.

Return only the review comment body. Start with this heading:

```md
## Codex Review - Automated
```

If there are blocking findings, list each one with severity, file path, line or
small range when available, impact, and the exact change needed.

If there are no blocking findings, say:

```md
## Codex Review - Automated

No blocking findings.
```
