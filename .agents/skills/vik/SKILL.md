---
name: vik
description: |
  Use Vik's `vik_issue` dynamic tool for configured tracker issue operations
  such as issue lookup, comment discovery, comment edits, state updates,
  attachment upload, and PR linking.
---

# Vik Issue Tracker Tool

Use this skill when a Vik app-server session needs to read or mutate the
configured tracker issue. The tool is tracker-agnostic: Vik routes each action
to the active provider from `WORKFLOW.md`, such as Linear or GitHub.

## Primary tool

Use the injected `vik_issue` dynamic tool. Pass the operation in `action`.

Common input fields:

```json
{
  "action": "get_issue",
  "issue_id": "vik issue id from the prompt"
}
```

Supported actions:

- `get_issue`: fetch the current issue by Vik issue id.
- `list_comments`: list tracker comments for an issue.
- `update_issue`: update state and labels.
- `create_comment`: create an issue comment.
- `update_comment`: update an existing comment by provider-specific comment id.
- `upload_attachment`: upload a workspace file through the tracker and post the
  returned link when supported.
- `link_pr`: link a pull request to the issue.

## Examples

Move an issue:

```json
{
  "action": "update_issue",
  "issue_id": "issue-id-from-prompt",
  "state": "In Progress"
}
```

Find and update a persistent workpad comment:

```json
{
  "action": "list_comments",
  "issue_id": "issue-id-from-prompt"
}
```

Then call:

```json
{
  "action": "update_comment",
  "comment_id": "comment-id-from-list-comments",
  "body": "replacement markdown"
}
```

Create a comment:

```json
{
  "action": "create_comment",
  "issue_id": "issue-id-from-prompt",
  "body": "comment markdown"
}
```

Link a pull request:

```json
{
  "action": "link_pr",
  "issue_id": "issue-id-from-prompt",
  "title": "Pull request title",
  "url": "https://github.com/owner/repo/pull/123"
}
```

Upload an attachment:

```json
{
  "action": "upload_attachment",
  "issue_id": "issue-id-from-prompt",
  "path": "relative/path/inside/workspace.log",
  "content_type": "text/plain"
}
```

## Usage rules

- Use the Vik issue id from the rendered prompt. Vik resolves that id to the
  provider-specific tracker id before calling Linear or GitHub.
- Use `list_comments` before updating a persistent workpad comment so you reuse
  the existing comment id instead of creating duplicates.
- Keep attachment paths inside the issue workspace. Vik rejects paths outside
  that workspace before calling the tracker provider.
- For GitHub, attachment upload returns an unsupported-operation error because
  the GitHub Issues API does not support uploaded attachments.
- Do not use raw tracker tokens or provider-specific shell helpers when
  `vik_issue` can perform the operation.
