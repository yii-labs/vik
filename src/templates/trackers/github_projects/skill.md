---
name: github-projects
description: Manage GitHub Project-backed issues for a Vik workflow with explicit gh commands.
---

# GitHub Projects

Use this skill for GitHub Project issue reads, comments, Status moves, and PR
links.

## Commands

Set `ISSUE_ID`, `PROJECT_ITEM_ID`, `PROJECT_OWNER`, and `PROJECT_NUMBER` from
the stage prompt.

- View issue: `gh issue view "$ISSUE_ID" --json number,title,body,labels,comments,url`
- Comment: `gh issue comment "$ISSUE_ID" --body "..."`
- Move Project Status: `gh project item-edit --id "$PROJECT_ITEM_ID" --project-id "$PROJECT_ID" --field-id "$STATUS_FIELD_ID" --single-select-option-id "$STATUS_OPTION_ID"`
- Find Project handles: `gh project view "$PROJECT_NUMBER" --owner "$PROJECT_OWNER" --format json --jq .id` and `gh project field-list "$PROJECT_NUMBER" --owner "$PROJECT_OWNER" --format json`.
- Link PR: include `Closes #$ISSUE_ID` in the PR body or run `gh pr edit <pr> --body-file <file>`.

## Read Before Work

Fetch current GitHub issue detail before changing code or Status:

`gh issue view "$ISSUE_ID" --json number,title,body,state,labels,comments,url,updatedAt`
