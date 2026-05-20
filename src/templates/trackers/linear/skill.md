---
name: linear-issues
description: Manage Linear issues for a Vik workflow with explicit Linear operations.
---

# Linear Issues

Use this skill for Linear issue reads, comments, state moves, and PR links.

## Intake

The generated workflow runs `sh ./scripts/linear-issues-json`. Set
`LINEAR_API_KEY` and `LINEAR_TEAM_KEY`, then edit the script for your Linear
team and workflow states.

Refresh this bundled tracker skill with `vik init --force` when you want the
latest template copy.

## Commands

- Read issue: Linear MCP `get_issue { id: "{{ issue.id }}" }`.
- Comment: Linear MCP `create_comment { issueId: "{{ issue.id }}", body: "..." }`.
- Move state: Linear MCP `update_issue`; first find the target state id with `get_workflow_states`.
- Attach PR: Linear MCP `create_attachment { issueId: "{{ issue.id }}", url: "<pr-url>", title: "<pr-title>" }`.

## Read Before Work

Fetch current Linear issue detail before changing code or state.
