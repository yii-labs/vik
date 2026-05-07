# Explicit Issue Management Commands

Vik does not own tracker issue management through built-in runtime tools. Workflow authors must put issue-fetching and issue-management commands in prompt sources, and Vik only consumes the issue JSON, matches `issue.state` to issue stages, and runs the selected prompt.

We rejected tracker-provider injection and the `vik_issue` dynamic tool because they hid workflow policy inside Vik. Explicit commands make the Workflow Definition the source of truth for how issues, comments, attachments, links, and state transitions are managed.
