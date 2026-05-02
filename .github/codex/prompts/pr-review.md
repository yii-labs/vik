# Codex PR Review Prompt

Review this pull request for merge-blocking defects only.

Focus on:

- correctness bugs, regressions, data loss, security issues, race conditions, and broken CI behavior
- missing tests only when changed behavior has no direct validation and the gap creates real merge risk

Do not flag style, naming, formatting, broad refactors, or optional improvements.

Write a GitHub comment-ready Markdown response:

- Start with `## Codex Review`.
- If there are no blocking findings, write `No blocking findings found.`
- For each finding, include severity, file path, line or function, impact, and a concrete fix.
- Keep the response concise and actionable.
