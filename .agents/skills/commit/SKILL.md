---
name: commit
description:
  Create a well-formed git commit from current changes. Use when asked to
  commit, finalize staged work, or prepare branch history for push.
---

# Commit

## Goals

- Commit only intended changes.
- Keep commit text English.
- Explain what changed, why, and how it was validated.

## Steps

1. Inspect scope:
   - `git status --short`
   - `git diff`
   - `git diff --staged`
2. Stage intended files. Include new files. Exclude logs, temp files, local
   secrets, target artifacts, and unrelated user changes.
3. Re-check staged diff:
   - `git diff --staged --stat`
   - `git diff --staged`
4. Choose conventional subject, with optional scope:
   - `feat(scope): ...`
   - `fix(scope): ...`
   - `docs(scope): ...`
   - `refactor(scope): ...`
   - `test(scope): ...`
   - `{chore|build|ci|perf|style}(scope?): ...`
5. Subject must be imperative, 72 chars or less, no trailing period.
6. Body must include:
   - `Summary:` bullets for what changed.
   - `Rationale:` bullets for why.
   - `Tests:` bullets with exact commands and results, or `not run` with reason.
7. Use a message file and `git commit -F <file>`.
8. After commit, show:
   - `git status --short`
   - `git log -1 --oneline`

## Template

```text
<type>(<scope>): <short imperative summary>

Summary:
- <what changed>

Rationale:
- <why it changed>

Tests:
- <command>: <result>
```

## Safety

- Do not commit non-English text.
- Do not commit secrets.
- Do not include unrelated files to make status clean.
- If staged diff and message disagree, fix staging or message before commit.
