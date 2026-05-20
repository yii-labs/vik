# Prepare Stage

Issue: `{{ issue.id }}`: `{{ issue.title }}`
Project status: `{{ issue.state }}`

You run a grill and context collection session for the issue.
Do not write production code in this stage.

This stage starts only from project Status `Digging`.
The successful state transitions from this stage are `In Progress` and `HITL`.

## Start

1. Read the issue body, comments, attached pull requests, branch links, and any existing `## Vik Workpad` comment
   by `gh issue view`.
2. Create one `## Vik Workpad` comment if none exists. Reuse and update the existing active workpad if present.
3. Do not create extra progress comments.
4. Update the same comment for every plan change.
5. Open and follow `{{ workflow_dir }}/.agents/skills/project-status/SKILL.md` before changing project Status.

## Grill and Context Collection

- Collect current facts before writing the plan: issue body, comments, active
  workpad, linked pull requests, branch links, review comments, CI state, docs,
  source, tests.
- If the skill `grill-me` or `grill-with-docs` is available, use its challenge
  style as a non-interactive self-grill. Do not enter interactive interview
  mode.
- Ask the important grill questions yourself. Answer them from collected
  context when the repo or tracker gives enough evidence.
- Choose the recommended answer when evidence makes it safe. Record important
  assumptions in `Notes`.
- If a safe answer cannot be inferred, record the question in `Confusions`,
  move project Status to `HITL`, and explain the blocker in the workpad.
- If `grill-with-docs` finds a domain term or documented-decision mismatch,
  record the mismatch in the workpad. Do not edit repo docs unless the issue
  asks for that doc change.
- If the skill `handoff` is available, use it only as a context compression aid.
  The durable handoff is the active `## Vik Workpad` comment.
- Do not rely on temp files or generated handoff files as the source of truth
  for the next stage. Summarize any useful handoff output back into the workpad
  before the state transition.

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

Move project Status to `In Progress` only when:

- Workpad exists and is current.
- Plan is actionable.
- Acceptance criteria are explicit.
- Validation checklist is explicit.
- No required information is missing.

Move project Status to `HITL` when a human decision is required, context is
missing, or proceeding would risk using private or ambiguous context.

Final response: completed actions and blockers only.
