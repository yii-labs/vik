---
name: github-projects
description: Manage GitHub Project-backed issues for a Vik workflow with explicit gh commands.
---

# GitHub Projects

Use this skill for GitHub Project issue reads, comments, Status moves, and PR
links.

## Intake

The generated workflow runs `sh ./scripts/github-project-items-json`. Set
`GITHUB_PROJECT_OWNER` and `GITHUB_PROJECT_NUMBER`, then edit the script for
your project field names, limits, and Status values.

Refresh this bundled tracker skill with `vik init --force` when you want the
latest template copy.

## Commands

Set `ISSUE_ID` and `PROJECT_ITEM_ID` from the stage prompt.

- View issue: `gh issue view "$ISSUE_ID" --json number,title,body,labels,comments,url`
- Comment: `gh issue comment "$ISSUE_ID" --body "..."`
- Move Project Status: `gh project item-edit --id "$PROJECT_ITEM_ID" --project-id "$GITHUB_PROJECT_ID" --field-id "$GITHUB_PROJECT_STATUS_FIELD_ID" --single-select-option-id "$GITHUB_PROJECT_STATUS_OPTION_ID"`
- Find Project handles: `gh project view "$GITHUB_PROJECT_NUMBER" --owner "$GITHUB_PROJECT_OWNER" --format json --jq .id` and `gh project field-list "$GITHUB_PROJECT_NUMBER" --owner "$GITHUB_PROJECT_OWNER" --format json`.
- Link PR: include `Closes #$ISSUE_ID` in the PR body or run `gh pr edit <pr> --body-file <file>`.

## Read Before Work

Fetch current GitHub issue detail before changing code or Status:

`gh issue view "$ISSUE_ID" --json number,title,body,state,labels,comments,url,updatedAt`
