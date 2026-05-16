# Prepare Stage

Issue: `{{ issue.id }}`: `{{ issue.title }}`

You prepare the issue for implementation.
Do not write production code in this stage.

The only successful state transition from this stage is `work`. If blocked before a safe plan exists, keep
the issue in its current state and record the blocker in the workpad.

## Start

1. Read the issue body, comments, attached pull requests, branch links, and any existing `## Vik Workpad` comment
   by `gh issue view`.
2. Create one `## Vik Workpad` comment if none exists. Reuse and update the existing active workpad if present.
3. Do not create extra progress comments.
4. Update the same comment for every plan change.
5. Move issue state to `work` only after the workpad is complete.

## Workpad Template

Keep this exact structure and update it in place:

```md
## Vik Workpad

### Plan

- [ ] 1. Parent task
  - [ ] 1.1 Child task

### Acceptance Criteria

- [ ] Criterion

### Validation

- [ ] targeted proof: `<command>`

### Notes

- <ISO 8601 timestamp> <short note>

### Confusions

- <only include when something was unclear>
```

## Required Work

- Convert issue requirements into a narrow checklist.
- Copy any issue-authored `Validation`, `Test Plan`, or `Testing` section into `Acceptance Criteria` and `Validation`.
- Add reproduction or proof strategy before implementation.
- Add expected final validation commands.
- Record any scope risks or confusing parts.
- Run a principal-style plan review and tighten scope before state transition.

## Finish

Move issue state to `work` only when:

- Workpad exists and is current.
- Plan is actionable.
- Acceptance criteria are explicit.
- Validation checklist is explicit.
- No required information is missing.

Final response: completed actions and blockers only.
