# Implement Stage

Issue: `{{ issue.id }}`: `{{ issue.title }}`
State: `{{ issue.state }}`

You implement or fix the issue.

## Start

1. Read the issue body, comments, attached pull requests, branch links, and the active `## Vik Workpad` comment by `gh issue view`.
2. Open and follow `.agents/skills/pull/SKILL.md` before code edits.
3. Record pull evidence in the workpad: source, result, resulting `HEAD`.
4. If there is already a PR linked to the issue, review its state and comments to understand the current implementation status and blockers before proceeding.
5. If applicable, use `TDD` style incremental development with a narrow green gate for each checklist item.

## Work Flow

- Reconcile the workpad before editing.
- Capture a concrete reproduction signal or current behavior proof.
- Use subagents for bounded sidecar research or review when available and
  useful. Main agent owns final decisions and tracker state.
- Implement only the workpad scope.
- Keep the workpad checklist current after each meaningful milestone.
- Add follow-up issues for meaningful out-of-scope work instead of expanding
  scope.


## Validation

- Execute every issue-authored `Validation`, `Test Plan`, or `Testing` item.
- Prefer targeted proof that directly demonstrates changed behavior.
- Run the repo-required checks before publish.
- For docs-only changes, use the docs-only narrow gate only when no code,
  config, workflow, prompt behavior, or runtime behavior changed.
- Revert temporary proof edits before commit.
- Record exact commands and results in the workpad.

## Commit And Push

1. Open and follow `.agents/skills/commit/SKILL.md`.
2. Open and follow `.agents/skills/push/SKILL.md`.
3. Ensure PR title/body reflect the full branch scope.
4. Ensure PR has label `vik`.
5. Link the PR to the tracker issue through an explicit tracker command or PR
   body link.
6. Update the workpad with final checklist status, commits, validation, PR URL,
   and risks.

## Finish

Move issue state to `review` only when:

- All planned work is complete.
- Acceptance criteria are checked.
- Required validation is green for latest commit.
- Branch is pushed.
- PR exists and metadata is current.
- Workpad is updated in place.

Final response: completed actions, validation, PR URL, blockers only.

## Blockers

- If you encounter blockers, document them in the workpad.
- Search on GitHub existing issues for the blocker.
  If an existing issue is relevant, link it in the workpad and comment on it with the blocker details.
  Otherwise, create a new issue with `vik` and `blocking` label, with the blocker details in its body, and link it in the workpad.
- NEVER write patch any changes irrelevant to resolve the blockers.
