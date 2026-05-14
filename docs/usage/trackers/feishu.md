# Feishu Base Issue Source

Vik does not talk to Feishu or Lark on its own. Listing records,
reading detail, writing comments, and moving state are all commands
you put in `issues.pull.command` or in stage prompt files.

This guide uses [lark-cli](https://github.com/larksuite/cli) and a
Feishu Base table as the issue source. It assumes you have read
[Get Started](../get-started.md) and have a working `workflow.yml`.

## Credentials

Install `lark-cli`:

```sh
npm install -g @larksuite/cli
```

The upstream installer also publishes agent skills. Install them when
your agent runtime should have local `lark-cli` skill docs:

```sh
npx skills add larksuite/cli -y -g
```

Configure the Feishu app and log in:

```sh
lark-cli config init --brand feishu
lark-cli auth login --domain base
lark-cli auth status --verify
```

For tighter scope control, request the exact Base scopes you need:

```sh
lark-cli auth login --scope "base:table:read base:field:read base:view:read base:record:read base:record:update"
lark-cli auth check --scope "base:table:read base:field:read base:view:read base:record:read base:record:update"
```

`vik doctor` only checks `workflow.yml`. It does not call Feishu,
check `lark-cli` auth, or verify Base table permissions. Your pull
command and prompt commands must fail clearly when auth or scopes are
wrong.

For detached daemons, start Vik from the same OS user that configured
`lark-cli`, or make sure that user can read the same `lark-cli`
profile and keychain entries.

## Design the Base table

Use one Base table as the issue list. The smallest useful schema is:

- `Title`: text field used as the Vik issue title.
- `State`: writable text or single-select field. Values must match
  `issue.stages.<stage>.when.state` exactly, including case.
- `Description`: optional long-text field for full issue context.
- `Workpad`: optional long-text field where agents can keep a running
  plan and validation notes.
- `Comments`: optional long-text field for append-only human or agent
  notes.
- `PR Links`: optional URL or text field for pull request links.

Field names are configurable. If you rename `Title` or `State`, update
the field names in your prompt snippets. For `+record-list`, use the
field IDs returned by `+field-list`, not display names.

`lark-cli` returns `_record_id` as record metadata. Do not create a
Base field named `_record_id`. Vik should use that record ID as
`issue.id`, because later prompt commands need the same ID for
`+record-get` and `+record-batch-update`.

Use a filtered Base view when possible. The view can hide done or
blocked records, sort by priority, and keep Vik's pull command small.

Discover tables, fields, and views before writing `workflow.yml`:

```sh
export FEISHU_BASE_TOKEN="<base_token>"
export FEISHU_TABLE_ID="<table_id>"
export FEISHU_VIEW_ID="<view_id>"
export FEISHU_TITLE_FIELD_ID="<title_field_id>"
export FEISHU_STATE_FIELD_ID="<state_field_id>"

lark-cli base +table-list --base-token "$FEISHU_BASE_TOKEN" --jq .
lark-cli base +field-list --base-token "$FEISHU_BASE_TOKEN" --table-id "$FEISHU_TABLE_ID" --jq .
lark-cli base +view-list --base-token "$FEISHU_BASE_TOKEN" --table-id "$FEISHU_TABLE_ID" --jq .
```

## Designing the issue pull command

`issues.pull.command` is a shell command Vik runs on a loop. It must
print one JSON array of issue objects to stdout. Each issue must
include at least:

- `id`: Feishu Base record ID, as a string.
- `title`: Base title field.
- `state`: Base state field. Match is case-sensitive.

Example using a filtered view. Keep the title field first and the
state field second; the `--jq` expression maps those two projected
columns by position. Substitute field IDs discovered with
`lark-cli base +field-list`; display names such as `Title` and `State`
are not valid `--field-id` values for normal Base tables.

```yaml
issues:
  pull:
    command: >-
      lark-cli base +record-list
      --base-token "$FEISHU_BASE_TOKEN"
      --table-id "$FEISHU_TABLE_ID"
      --view-id "$FEISHU_VIEW_ID"
      --field-id "$FEISHU_TITLE_FIELD_ID"
      --field-id "$FEISHU_STATE_FIELD_ID"
      --limit 50
      --format json
      --jq '
        [
          range(0; (.record_id_list | length)) as $i
          | {
              id: (.record_id_list[$i] | tostring),
              title: (.data[$i][0] | tostring),
              state: (.data[$i][1] | tostring)
            }
        ]
      '
    idle_sec: 10
```

If you do not use a filtered view, remove the `--view-id` line and
make the Base table itself small enough for every pull cycle.

Tips:

- Use `--field-id` repeatedly to keep output small.
- Use `--limit 50` or another explicit cap. `+record-list` accepts
  values from 1 to 200.
- If the result can be more than one page, tighten the view filter
  before running Vik. Do not make intake depend on unbounded local
  pagination.
- Run the exact command in your shell before putting it in
  `workflow.yml`.

## Reading issue detail in prompts

Stage prompts can render Vik template values directly:

```text
You are working on Feishu Base record {{ issue.id }}: {{ issue.title }}.
State: {{ issue.state }}
Workdir: {{ cwd }}
```

The pull command only carries the small fields you projected. Fetch
the full Base record at the start of each stage:

```sh
lark-cli base +record-get \
  --base-token "$FEISHU_BASE_TOKEN" \
  --table-id "$FEISHU_TABLE_ID" \
  --record-id "{{ issue.id }}" \
  --jq .
```

If your table has large attachment or rich-text fields, project only
the fields a stage needs.

## Managing state from prompts

Vik never updates Feishu. Your prompt files must tell the agent how
to move Base records between states.

Move a record to `review`:

```sh
payload=$(
  jq -n \
    --arg id "{{ issue.id }}" \
    --arg field "${FEISHU_STATE_FIELD:-State}" \
    --arg state "review" \
    '{record_id_list: [$id], patch: {($field): $state}}'
)

lark-cli base +record-batch-update \
  --base-token "$FEISHU_BASE_TOKEN" \
  --table-id "$FEISHU_TABLE_ID" \
  --json "$payload"
```

For single-select fields, make sure the target option already exists
in Base. Read field structure first when the patch fails:

```sh
lark-cli base +field-list \
  --base-token "$FEISHU_BASE_TOKEN" \
  --table-id "$FEISHU_TABLE_ID" \
  --jq .
```

## Common prompt operations

### Write a Workpad field

Use this when your table has a long-text field such as `Workpad`.
Read the existing record first, preserve the current field text, then
write the full new field body back.

```sh
record_json=$(
  lark-cli base +record-get \
    --base-token "$FEISHU_BASE_TOKEN" \
    --table-id "$FEISHU_TABLE_ID" \
    --record-id "{{ issue.id }}" \
    --jq .
)

existing_workpad=$(
  printf '%s' "$record_json" |
    jq -r --arg field "${FEISHU_WORKPAD_FIELD:-Workpad}" \
      '.fields[$field] // "" | if type == "string" then . else tostring end'
)

workpad_entry='## Vik Workpad

### Plan

- [x] Read the record.
- [ ] Implement the issue.
'

if [ -n "$existing_workpad" ]; then
  workpad_body="${existing_workpad}

${workpad_entry}"
else
  workpad_body="$workpad_entry"
fi

payload=$(
  jq -n \
    --arg id "{{ issue.id }}" \
    --arg field "${FEISHU_WORKPAD_FIELD:-Workpad}" \
    --arg body "$workpad_body" \
    '{record_id_list: [$id], patch: {($field): $body}}'
)

lark-cli base +record-batch-update \
  --base-token "$FEISHU_BASE_TOKEN" \
  --table-id "$FEISHU_TABLE_ID" \
  --json "$payload"
```

### Write a comment field

Base does not need to model comments as separate records for Vik to
work. The simple approach is one long-text field named `Comments`.
Append locally, then update the whole field:

```sh
record_json=$(
  lark-cli base +record-get \
    --base-token "$FEISHU_BASE_TOKEN" \
    --table-id "$FEISHU_TABLE_ID" \
    --record-id "{{ issue.id }}" \
    --jq .
)

existing_comments=$(
  printf '%s' "$record_json" |
    jq -r --arg field "${FEISHU_COMMENTS_FIELD:-Comments}" \
      '.fields[$field] // "" | if type == "string" then . else tostring end'
)

comment_entry='2026-05-14 12:00 UTC Agent posted plan and moved state to work.'

if [ -n "$existing_comments" ]; then
  comment_body="${existing_comments}

${comment_entry}"
else
  comment_body="$comment_entry"
fi

payload=$(
  jq -n \
    --arg id "{{ issue.id }}" \
    --arg field "${FEISHU_COMMENTS_FIELD:-Comments}" \
    --arg body "$comment_body" \
    '{record_id_list: [$id], patch: {($field): $body}}'
)

lark-cli base +record-batch-update \
  --base-token "$FEISHU_BASE_TOKEN" \
  --table-id "$FEISHU_TABLE_ID" \
  --json "$payload"
```

### Store a pull request link

```sh
payload=$(
  jq -n \
    --arg id "{{ issue.id }}" \
    --arg field "${FEISHU_PR_LINKS_FIELD:-PR Links}" \
    --arg url "$PR_URL" \
    '{record_id_list: [$id], patch: {($field): $url}}'
)

lark-cli base +record-batch-update \
  --base-token "$FEISHU_BASE_TOKEN" \
  --table-id "$FEISHU_TABLE_ID" \
  --json "$payload"
```

## Sanity checks before you run Vik

```sh
# 1. CLI is installed and can see Base commands.
lark-cli base --help
lark-cli base +record-list --help
lark-cli base +record-batch-update --help

# 2. Auth and scopes work for this OS user.
lark-cli auth status --verify
lark-cli auth check --scope "base:table:read base:field:read base:view:read base:record:read base:record:update"

# 3. Field discovery works.
lark-cli base +field-list --base-token "$FEISHU_BASE_TOKEN" --table-id "$FEISHU_TABLE_ID" --jq .

# 4. Pull command prints a JSON array.
lark-cli base +record-list \
  --base-token "$FEISHU_BASE_TOKEN" \
  --table-id "$FEISHU_TABLE_ID" \
  --view-id "$FEISHU_VIEW_ID" \
  --field-id "$FEISHU_TITLE_FIELD_ID" \
  --field-id "$FEISHU_STATE_FIELD_ID" \
  --limit 5 \
  --format json \
  --jq '[range(0; (.record_id_list | length)) as $i | {id: (.record_id_list[$i] | tostring), title: (.data[$i][0] | tostring), state: (.data[$i][1] | tostring)}]'

# 5. Vik schema is happy. This does not verify Feishu auth.
vik doctor ./workflow.yml
```

## Related

- [Get Started](../get-started.md)
- [Configuration](../configuration.md)
- [GitHub Issue Source](github.md)
- [Linear Issue Source](linear.md)
