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

For code, workflow, prompt behavior, config, or broad repo changes, run:

```sh
cargo run --locked -- doctor --json ./workflow.yml
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features
git diff --check
```

For docs-only or skill-only changes, run:

```sh
cargo run --locked -- --help
cargo run --locked -- doctor --json ./workflow.yml
git diff --check
```

Also run any issue-provided validation. Do not downgrade explicit issue tests.

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
   - add label `vik`
9. Link PR to tracker issue through explicit tracker command or PR body.
10. Check remote status:
    - `gh pr view --json url,state,mergeStateStatus,reviewDecision,headRefOid`
    - `gh pr checks`
11. Update workpad with PR URL, commit, validation, and remote status.

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
- Do not move tracker state to review if local validation failed.
- Do not hide pending or failed checks.
