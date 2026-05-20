---
name: github-issues
description: Manage GitHub Issues for a Vik workflow with explicit gh commands.
---

# GitHub Issues

Use this skill for GitHub issue reads, comments, state labels, and PR links.

## Intake

The generated workflow runs `sh ./scripts/github-issues-json`. Edit that script
for your repository labels, blocked label, limit, and sort order.

Refresh this bundled tracker skill with `vik init --force` when you want the
latest template copy.

## Commands

Set `ISSUE_ID` to the issue id shown in the stage prompt.

- View issue: `gh issue view "$ISSUE_ID" --json number,title,body,labels,comments,url`
- Comment: `gh issue comment "$ISSUE_ID" --body "..."`
- Move label state: `gh issue edit "$ISSUE_ID" --remove-label <old-state> --add-label <new-state>`
- Link PR: include `Closes #$ISSUE_ID` in the PR body or run `gh pr edit <pr> --body-file <file>`.

## Read Before Work

Fetch current GitHub issue detail before changing code or state:

`gh issue view "$ISSUE_ID" --json number,title,body,state,labels,comments,url,updatedAt`
