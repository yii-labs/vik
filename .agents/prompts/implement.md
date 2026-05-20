# Implement Stage

Issue: `{{ issue.id }}`: `{{ issue.title }}`
Project status: `{{ issue.state }}`

You implement or fix the issue.

## Start

1. Read the issue body, comments, attached pull requests, branch links, and the active `## Vik Workpad` comment by `gh issue view`.
2. Open and follow `.agents/skills/pull/SKILL.md` before code edits.
3. Open and follow `{{ workflow_dir }}/.agents/skills/project-status/SKILL.md` before changing project Status.
4. Record pull evidence in the workpad: source, result, resulting `HEAD`.
5. If applicable, use `TDD` style incremental development with a narrow green gate for each checklist item.

## PR feedback sweep protocol (required)

When an issue has an attached PR, run this protocol before moving to `Reviewing`:

1. Identify the PR number from issue links/attachments.
2. Gather feedback from all channels:
   - Top-level PR comments (`gh pr view --comments`).
   - Inline review comments (`gh api repos/<owner>/<repo>/pulls/<pr>/comments`).
   - Review summaries/states (`gh pr view --json reviews`).
3. Treat every actionable reviewer comment (human or bot), including inline review comments, as blocking until one of these is true:
   - code/test/docs updated to address it, or
   - explicit, justified pushback reply is posted on that thread.
4. Update the workpad plan/checklist to include each feedback item and its resolution status.
5. Re-run validation after feedback-driven changes and push updates.
6. Repeat this sweep until there are no outstanding actionable comments.

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
4. GitHub Project `4` is the issue state tracker. Keep issue state changes in
   project Status, not labels.
5. Link the PR to the issue with GitHub's native closing keyword in the PR
   body:

   ```md
   Closes #{{ issue.id }}
   ```

   For same-repo PRs merged into the default branch, GitHub uses this link to
   close the issue after merge.

6. Confirm GitHub detected the closing link before moving project Status to
   `Reviewing`:

   ```sh
   gh pr view --json closingIssuesReferences --jq '.closingIssuesReferences[].number'
   ```

   The output must include `{{ issue.id }}`.

7. Update the workpad with final checklist status, commits, validation, PR URL,
   and risks.

## Finish

Move project Status to `Reviewing` only when:

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
