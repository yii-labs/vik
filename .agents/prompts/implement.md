# Implement Stage

Issue: `{{ issue.id }}`
Stage: `{{ stage.name }}`
State: `{{ issue.state }}`
Workdir: `{{ cwd }}`

You implement the issue. This prompt handles both `work` and `rework` states.

Work only in `{{ cwd }}`. Do not touch paths outside this issue workspace.

## Start

1. `cd {{ cwd }}`
2. Fetch the latest tracker issue before work:

   ```sh
   gh issue view {{ issue.id }} --json number,title,body,state,labels,assignees,comments,url,updatedAt
   ```

3. Read the fresh issue body, comments, attached pull requests, branch links, and the
   active `## Vik Workpad`.
4. Read relevant repo docs before edits:
   - `docs/development/index.md`
   - `docs/development/checks.md`
   - `docs/development/pull-requests.md`
   - `docs/development/code-conventions.md`
5. Open and follow `.agents/skills/pull/SKILL.md` before code edits.
6. Record pull evidence in the workpad: source, result, resulting `HEAD`.

If `.agents/skills/pull/SKILL.md` is missing, run the equivalent:

```sh
git fetch origin
git merge origin/main
```

## Normal `work` Flow

- Reconcile the workpad before editing.
- Capture a concrete reproduction signal or current behavior proof.
- Use subagents for bounded sidecar research or review when available and
  useful. Main agent owns final decisions and tracker state.
- Implement only the workpad scope.
- Keep the workpad checklist current after each meaningful milestone.
- Add follow-up issues for meaningful out-of-scope work instead of expanding
  scope.
- Keep all committed source, config, docs, prompts, and commit messages in
  English.

## `rework` Flow

When `{{ issue.state }}` is `rework`, treat the task as a full approach reset:

- Reread the issue, workpad, PR comments, inline review comments, and CI state.
- Identify what must change this attempt.
- Close or supersede stale PR state when it no longer matches the issue.
- Reset stale workpad sections instead of layering a patch plan on top.
- Create or switch to a fresh issue branch from `origin/main` when the old
  branch is not reusable.
- Run the normal implementation flow after the reset.

## Validation

- Execute every issue-authored `Validation`, `Test Plan`, or `Testing` item.
- Prefer targeted proof that directly demonstrates changed behavior.
- Run the repo-required checks from `docs/development/checks.md` before publish.
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

## Note

If you encounter blockers, document them in the workpad and report it by creating new issue with `vik` label.
Never write patch any changes irrelevant to the issue scope.
