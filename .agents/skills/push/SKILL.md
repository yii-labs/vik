---
name: push
description:
  Validate, push current branch, and create or update its GitHub pull request.
  Use when publishing implementation work or preparing review handoff.
---

# Push

## Goals

- Push committed branch changes safely.
- Create or refresh the PR.
- Keep PR metadata aligned with issue scope.
- Surface local and remote validation state.

## Prerequisites

- Working tree changes are committed.
- `gh auth status --active --hostname github.com` succeeds for PR operations.
- Branch includes latest `origin/main`; use the pull skill first when stale.

## Validation

Run validation appropriate for the current branch scope before every push.
Include any issue-provided validation. Do not downgrade explicit issue tests.
Do not publish if required validation failed.

## Steps

1. Inspect:
   - `git status --short --branch`
   - `git log -1 --oneline`
   - `git branch --show-current`
2. Confirm branch is not `main` unless user explicitly wants to publish from
   `main`.
3. Confirm branch includes latest `origin/main`; otherwise run pull skill and
   rerun validation.
4. Run validation before every push attempt.
5. Push:
   - `git push -u origin HEAD`
6. If rejected for non-fast-forward:
   - run pull skill
   - rerun validation
   - push again
   - use `--force-with-lease` only after history was rewritten locally
7. If rejected for auth or permissions:
   - try `gh auth status --active --hostname github.com`
   - use a one-off HTTPS push URL from `gh repo view --json url -q .url` only
     when it fixes auth without rewriting persistent remotes
   - record exact failure if still blocked
8. Create or update PR:
   - use clear title covering full branch scope
   - body includes issue, summary, validation, risks
   - refresh body on every update; do not keep stale text
9. Link PR to the issue through an explicit command or PR body when needed.
10. Check remote status:
    - `gh pr view --json url,state,mergeStateStatus,reviewDecision,headRefOid`
    - `gh pr checks`
11. Record PR URL, commit, validation, and remote status in the requested
    handoff location when applicable.

## PR Body Shape

```md
## Issue
closes <issue identifier or link>

## Summary
- <full scope>

## Validation
- <command>: <result>

## Risks
- <risk or "None known">
```

## Safety

- Do not use plain `--force`.
- Do not leave PR in draft unless user asked for draft.
- Do not hide pending or failed checks.
