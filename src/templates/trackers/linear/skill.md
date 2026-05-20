---
name: linear-issues
description: Manage Linear issues for a Vik workflow with explicit Linear operations.
---

# Linear Issues

Use this skill for Linear issue reads, comments, state moves, and PR links.

## Commands

Set `LINEAR_ISSUE_ID` to the issue id shown in the stage prompt.

- Read issue: Linear MCP `get_issue { id: "$LINEAR_ISSUE_ID" }`.
- Comment: Linear MCP `create_comment { issueId: "$LINEAR_ISSUE_ID", body: "..." }`.
- Move state: Linear MCP `update_issue`; first find the target state id with `get_workflow_states`.
- Attach PR: Linear MCP `create_attachment { issueId: "$LINEAR_ISSUE_ID", url: "<pr-url>", title: "<pr-title>" }`.

## Read Before Work

Fetch current Linear issue detail before changing code or state.
