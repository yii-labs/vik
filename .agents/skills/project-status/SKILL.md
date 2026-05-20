---
name: project-status
description:
  Move Vik GitHub issues through GitHub Project 4 Status values.
  Use before changing tracker state in Vik workflow prompts.
---

# Project Status

## Scope

- Project: `https://github.com/orgs/yii-labs/projects/4`
- Repo: `yii-labs/vik`
- Status field values:
  - `Todo`: default, not ready for automation.
  - `Digging`: AI collects context and writes plan.
  - `HITL`: human input required before automation continues.
  - `In Progress`: AI has enough context and is coding.
  - `Reviewing`: work is done and needs review.
  - `Merging`: approved work is ready to land.
  - `Done`: final state after merge and issue close.

Vik handles only `Digging`, `In Progress`, and `Merging`.
Never start work from `Todo`, `HITL`, `Reviewing`, or `Done`.

## Auth

Project reads and writes require GitHub project scope:

```sh
gh auth refresh -s project
```

If `gh` reports missing `read:project` or `project` scope, stop and record the
auth blocker in the workpad.

## Find Project Handles

```sh
project_id=$(gh project view 4 --owner yii-labs --format json --jq .id)
status_field_id=$(gh project field-list 4 --owner yii-labs --format json --jq '.fields[] | select(.name == "Status") | .id')
```

Find the option id for the target status:

```sh
target_status="In Progress"
status_option_id=$(gh project field-list 4 --owner yii-labs --format json --jq ".fields[] | select(.name == \"Status\") | .options[] | select(.name == \"$target_status\") | .id")
```

Find the project item for this issue:

```sh
issue_id="<issue-id>"
item_id=$(
  gh project item-list 4 --owner yii-labs --limit 200 --format json \
    --query "repo:yii-labs/vik is:issue" \
    --jq ".items[] | select((.content.type // .type) == \"Issue\") | select((.content.repository // .content.repository.nameWithOwner? // .repository // \"\") == \"yii-labs/vik\") | select(((.content.number? // .number? // ((.content.url // .url // \"\") | split(\"/\")[-1])) | tostring) == \"$issue_id\") | .id"
)
```

If `item_id` is empty, add the issue to the project, then resolve `item_id`
again:

```sh
gh project item-add 4 --owner yii-labs --url "https://github.com/yii-labs/vik/issues/$issue_id"
item_id=$(
  gh project item-list 4 --owner yii-labs --limit 200 --format json \
    --query "repo:yii-labs/vik is:issue" \
    --jq ".items[] | select((.content.type // .type) == \"Issue\") | select((.content.repository // .content.repository.nameWithOwner? // .repository // \"\") == \"yii-labs/vik\") | select(((.content.number? // .number? // ((.content.url // .url // \"\") | split(\"/\")[-1])) | tostring) == \"$issue_id\") | .id"
)
```

## Move Status

Use exact Status names only.

```sh
gh project item-edit \
  --id "$item_id" \
  --project-id "$project_id" \
  --field-id "$status_field_id" \
  --single-select-option-id "$status_option_id"
```

## Workflow Rules

- `Digging` may move to `In Progress` when plan, acceptance criteria, and
  validation are ready.
- `Digging` may move to `HITL` when a human decision is required, context is
  missing, or proceeding would risk using private or ambiguous context.
- `In Progress` may move to `Reviewing` only after implementation, validation,
  push, PR metadata, and workpad updates are complete.
- `Reviewing` is human-owned. Agents should wait until project Status becomes
  `Merging`.
- `Done` is automation-owned. Do not move an issue to `Done` manually; confirm
  the project automation set it after merge.

Record every Status change in the active `## Vik Workpad` comment with command
result and timestamp.
