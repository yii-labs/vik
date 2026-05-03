---
name: push
description:
  Push current branch changes to origin and create or update the corresponding
  pull request; use when asked to push, publish updates, or create pull request.
---

# Push

## Prerequisites

- `gh` CLI is installed and available in `PATH`.
- `gh auth status` succeeds for GitHub operations in this repo.
- Working tree changes are committed before publishing.

## Goals

- Push current branch changes to `origin` safely.
- Create a PR if none exists for the branch, otherwise update the existing PR.
- Run the repo-required local checks before every push attempt.
- Keep PR metadata aligned with the Vik workflow contract.
- Surface remote CI/review status after publishing.

## Related Skills

- `pull`: use this when push is rejected or sync is not clean (non-fast-forward,
  rebase conflict risk, or stale branch).

## Steps

1. Identify current branch, `HEAD`, dirty state, upstream, and remote state.
2. Confirm intended changes are committed. If not, use the `commit` skill first.
3. Run validation for the changed scope before every push attempt.
   - For Rust code, workflow config, scripts, or broad repo changes, run the
     full Vik gate:
     - `LINEAR_API_KEY=ci-placeholder cargo run --locked -p vik-cli -- ./WORKFLOW.md --check`
     - `cargo fmt --all -- --check`
     - `cargo clippy --locked --workspace --all-targets --all-features -- -D warnings`
     - `cargo test --locked --workspace --all-features`
   - For docs/skill-only changes, run `git diff --check` and any more specific
     check that applies. Do not skip ticket-provided `Validation`, `Test Plan`,
     or `Testing` requirements.
4. Before final publish or Human Review handoff, confirm the branch includes the
   latest `origin/main`. If not, use the `pull` skill, resolve conflicts, and
   rerun validation before pushing.
5. Push branch to `origin` with upstream tracking if needed.
6. If push is not clean/rejected:
   - If the failure is a non-fast-forward or sync problem, run the `pull`
     skill to rebase onto latest `origin/main`, resolve conflicts, and rerun
     validation.
   - Push again; use `--force-with-lease` only when history was rewritten.
   - If the failure is due to auth, permissions, or workflow restrictions on
     the configured remote, try safe GitHub fallback auth first. Prefer a
     one-off HTTPS push URL from `gh repo view` over rewriting persistent
     remotes. If all fallback strategies fail, surface the exact error.
7. Ensure a PR exists for the branch:
   - If no PR exists, create one.
   - If a PR exists and is open, update it.
   - If branch is tied to a closed/merged PR, create a new branch + PR.
   - Write a proper PR title that clearly describes the change outcome.
   - For branch updates, explicitly reconsider whether current PR title still
     matches the latest scope; update it if it no longer does.
   - Do not leave the PR in draft unless the user explicitly asked for a draft;
     repo review checks only run for non-draft PRs.
8. Write/update PR body explicitly:
   - Describe full PR scope, validation run, and known risks.
   - If PR already exists, refresh body content so it reflects the total PR
     scope (all intended work on the branch), not just the newest commits,
     including newly added work, removed work, or changed approach.
   - Do not reuse stale description text from earlier iterations.
9. Ensure required PR metadata exists:
   - Add the `vik` label if missing.
   - Attach/link the PR URL to the Linear issue when issue context is known.
10. After publishing, inspect remote checks for the latest PR head.
   - Run `gh pr checks --watch` when handing off to Human Review or when the
     user asks for a complete publish/check cycle.
   - If checks fail, inspect logs, fix, commit, rerun local validation, push,
     and re-check.
   - If checks are pending and the user only asked to publish, report pending
     checks with the PR URL.
11. If Codex review comments or human review comments appear before handoff,
    handle them with the workflow PR feedback sweep before moving the issue to
    Human Review.
12. Reply with PR URL, latest commit, local validation commands run, and remote
    check status.

## Commands

```sh
# Identify branch
branch=$(git branch --show-current)
head_sha=$(git rev-parse --short HEAD)
git status --short
git remote -v

# Full Vik validation gate for Rust/workflow/broad repo changes.
LINEAR_API_KEY=ci-placeholder cargo run --locked -p vik-cli -- ./WORKFLOW.md --check
cargo fmt --all -- --check
cargo clippy --locked --workspace --all-targets --all-features -- -D warnings
cargo test --locked --workspace --all-features

# Initial push: respect the current origin remote. If this fails, classify the
# error before retrying.
git push -u origin HEAD

# If that failed because the remote moved, use the pull skill. After
# pull-skill resolution and re-validation, retry the normal push:
git push -u origin HEAD

# If origin push fails due to auth/permission, try a temporary HTTPS URL from
# gh before declaring GitHub blocked. Run only for auth/permission failures.
# Do not persistently rewrite remotes.
repo_url=$(gh repo view --json url -q .url)
git push "$repo_url" "HEAD:${branch}"

# Only if history was rewritten locally:
git push --force-with-lease origin HEAD

# Ensure a PR exists (create only if missing)
pr_state=$(gh pr view --json state -q .state 2>/dev/null || true)
if [ "$pr_state" = "MERGED" ] || [ "$pr_state" = "CLOSED" ]; then
  echo "Current branch is tied to a closed PR; create a new branch + PR." >&2
  exit 1
fi

# Write a clear, human-friendly title that summarizes the shipped change.
pr_title="<clear PR title written for this change>"
pr_body_file=$(mktemp)
cat >"$pr_body_file" <<'EOF'
## Summary
- <full PR scope>

## Validation
- <commands run and result>

## Risks
- <known risks or "None known">
EOF

if [ -z "$pr_state" ]; then
  gh pr create --title "$pr_title" --body-file "$pr_body_file"
else
  # Reconsider title on every branch update; edit if scope shifted.
  gh pr edit --title "$pr_title" --body-file "$pr_body_file"
fi
rm -f "$pr_body_file"

# Required PR metadata.
gh pr edit --add-label vik

# Remote check status. For handoff, wait until green or failed.
gh pr checks --watch

# Show PR URL for the reply
gh pr view --json url -q .url
```

## Notes

- Do not use `--force`; only use `--force-with-lease` as the last resort.
- Distinguish sync problems from remote auth/permission problems:
  - Use the `pull` skill for non-fast-forward or stale-branch issues.
  - Surface auth, permissions, or workflow restrictions directly instead of
    changing remotes or protocols.
