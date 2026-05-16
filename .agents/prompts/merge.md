# Merge Stage

Issue: `{{ issue.id }}`: `{{ issue.title }}`

You are landing an approved PR.

## Start

1. Open and follow `.agents/skills/land/SKILL.md`.
2. Do not merge by hand without following the land skill.
3. Keep the existing `## Vik Workpad` current.

## Required Checks

- Clean working tree or committed local changes.
- PR branch synced with `origin/main`.
- No merge conflicts.
- Required local validation green.
- Remote checks green.
- No actionable review comments remain.
- PR title/body/label current.

## Finish

After successful merge:

- Move tracker issue to done or close it using explicit tracker commands.
- Update the workpad with merge commit or PR merge evidence.
- Do not delete remote branches unless repo policy or land skill says to.
- Remove the local `{{ issue.workdir }}` workspace.

Final response: merged PR, final issue state, validation/check status,
blockers only.
