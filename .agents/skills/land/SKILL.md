---
name: land
description:
  Land a PR by resolving conflicts, handling review feedback, waiting for CI,
  and squash-merging when safe.
---

# Land

## Goals

- Merge only latest approved PR head.
- Keep CI green.
- Address or explicitly push back on all actionable feedback.
- Avoid stale-head merges.

## Preconditions

- `gh` is authenticated.
- Current branch is the PR branch.
- Working tree is clean or local changes are committed.

## Steps

1. Inspect:
   - `git status --short --branch`
   - `gh pr view --json number,url,title,body,headRefOid,mergeStateStatus,reviewDecision`
2. Run the required local validation for current branch scope.
3. If uncommitted changes exist, use commit skill, then push skill.
4. If PR has conflicts or is behind, use pull skill, rerun validation, then push
   skill.
5. Run review sweep:
   - top-level PR comments
   - inline review comments
   - review summaries
   - bot review comments
6. For each actionable comment, choose one:
   - accept and fix
   - ask concise clarification
   - push back with rationale
7. All GitHub comments by this agent must start with `[codex]`.
8. After fixes, commit, push, rerun validation, and post one concise update with
   commit SHA and tests.
9. Watch remote checks and feedback:
   - `python3 .agents/skills/land/land_watch.py`
10. If watcher reports feedback, address it and restart step 9.
11. If watcher reports failed checks, inspect logs, fix, commit, push, and
    restart step 9.
12. When checks are green and feedback is clear, merge:

```sh
pr_number=$(gh pr view --json number -q .number)
head_sha=$(gh pr view --json headRefOid -q .headRefOid)
title=$(gh pr view --json title -q .title)
body=$(gh pr view --json body -q .body)
gh pr merge "$pr_number" --squash --delete-branch --match-head-commit "$head_sha" --subject "$title" --body "$body"
```

13. Confirm merged:
    - `gh pr view --json state,mergeCommit,url`
14. Update tracker issue and workpad with merge evidence.

## Failure Handling

- Exit code `2` from watcher: review feedback exists.
- Exit code `3` from watcher: checks failed or never appeared.
- Exit code `4` from watcher: PR head changed.
- Exit code `5` from watcher: PR has conflicts.

For CI failure:

- `gh pr checks`
- `gh run list --branch "$(git branch --show-current)"`
- `gh run view <run-id> --log`

For stale PR head:

- fetch and inspect remote branch
- rerun validation
- push a real author commit when needed to retrigger CI

## Safety

- Do not enable auto-merge.
- Do not merge with unresolved human feedback.
- Do not merge if checks are pending or failed.
- Do not merge a PR head different from the one just validated.
