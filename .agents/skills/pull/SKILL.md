---
name: pull
description:
  Sync current branch with origin/main using the repo's merge-based workflow.
  Use before implementation and before handoff.
---

# Pull

## Goals

- Bring latest `origin/main` into the current branch.
- Preserve user work.
- Record exact sync evidence for the issue workpad.

## Steps

1. Inspect current state:
   - `git status --short --branch`
   - `git branch --show-current`
   - `git remote -v`
2. If uncommitted changes exist, commit intended work first or stop and record
   why sync is unsafe.
3. Fetch:
   - `git fetch origin`
4. Merge latest main:
   - `git merge origin/main`
5. If conflicts appear:
   - Read both sides before editing.
   - Preserve current branch intent and upstream invariants.
   - Resolve one logical batch at a time.
   - Run `git diff --check`.
   - Finish merge with `git add <files>` and `git commit` if Git did not create
     the merge commit automatically.
6. Run validation appropriate for changed files.
7. Record workpad evidence:
   - merge source: `origin/main`
   - result: `clean` or `conflicts resolved`
   - resulting `HEAD` short SHA
   - validation commands and results

## Conflict Guidance

- Inspect context first:
  - `git status`
  - `git diff`
  - `git diff :1:path :2:path`
  - `git diff :1:path :3:path`
- Do not choose ours/theirs blindly.
- Prefer the smallest semantic resolution.
- For generated files, resolve source first, then regenerate.
- Ask only when product intent cannot be inferred from code, tests, docs, or
  issue context.

## Safety

- Do not run `git reset --hard`.
- Do not discard user changes.
- Do not hide conflicts or failing validation.
