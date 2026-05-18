# GitHub Issue Source

Vik does not talk to GitHub on its own. Everything that touches the
tracker — listing issues, reading details, leaving comments, moving
state — is something _you_ tell Vik to do, either through the
`issues.pull.command` shell snippet or through commands written into
your stage prompt sources.

This guide shows the patterns we use in practice with the GitHub CLI
(`gh`) and `jq`. It assumes you have read [Get Started](../get-started.md)
and have a working `workflow.yml`.

## Credentials

The GitHub CLI handles auth for you. Make sure it is logged in for
the host you use:

```sh
gh auth status --active --hostname github.com
gh auth setup-git --hostname github.com
```

Alternatives, in order of preference:

- `gh auth login` for interactive setup (recommended).
- `GH_TOKEN` or `GITHUB_TOKEN` exported in the daemon's environment.
  Use a fine-grained token with the minimum scopes needed (`issues`,
  `pull_requests`, optionally `contents`).

> Never `echo`, `cat`, or commit a token. If you set `GH_TOKEN` in a
> shell file, make sure that file is git-ignored.

`vik doctor` only checks `workflow.yml`. It does not call GitHub —
your pull command and prompts are responsible for failing loudly when
auth is broken.

## Designing the issue pull command

`issues.pull.command` is a shell command Vik runs on a loop. It must
print one JSON array of issue objects to stdout. Each issue must
include at least:

- `id` — the GitHub issue number, **as a string** (Vik uses this for
  workspace folder names).
- `title` — the GitHub issue title.
- `state` — the value Vik will match against `issue.stages.<stage>.when.state`.
  Match is case-sensitive.

GitHub does not have a built-in "workflow state" field. Use one of
these conventions:

### Pattern: state from labels

This is the pattern used by Vik's own workflow. You add labels like
`todo`, `work`, `review` to issues; the pull command picks the active
state label and emits it as `state`.

```yaml
issues:
  pull:
    command: >-
      gh issue list --label "vik" --state "open" --limit 50
      --search 'label:todo,label:work,label:review -label:blocked sort:created-asc'
      --json number,title,labels
      --jq '
        [
          .[]
          | ([.labels[].name]
              | map(select(. == "todo" or . == "work" or . == "review"))
            ) as $states
          | select($states | length == 1)
          | { id: (.number | tostring), title: .title, state: $states[0] }
        ]
      '
    idle_sec: 5
```

What this does, step by step:

1. `gh issue list` filters to open issues with the `vik` label.
2. `--search` further restricts to issues that carry exactly one of
   the workflow state labels and are not blocked.
3. `--json` selects the raw fields we need.
4. `--jq` reshapes each issue into Vik's required `{id, title, state}`
   shape, dropping any issue that has zero or more than one state
   label (which would be ambiguous).

### Pattern: state from a project field

If you use GitHub Projects (v2), pull from there instead so the
project board is the source of truth:

```sh
gh project item-list <project-number> --owner <org> --format json --limit 100 \
  | jq '
    [
      .items[]
      | select(.content.type == "Issue")
      | {
          id: (.content.number | tostring),
          title: .content.title,
          state: .status
        }
    ]
  '
```

Replace `.status` with whatever the field is called in your project.
You can find the exact key name with `gh project field-list <number> --owner <org>`.

### Tips for any pull command

- **Limit the result set.** Vik runs this every cycle. `--limit 50`
  or a tight `--search` query keeps you well under GitHub's rate
  limit.
- **Sort deterministically** (`sort:created-asc`, `sort:updated-desc`,
  etc.) so the same issue is not "first" on every cycle if it
  matters to your hooks.
- **Test the command by hand.** Run the exact string in your shell
  and confirm the output is a JSON array — not an object, not
  newline-delimited objects.
- **Pick `idle_sec` to match your tracker.** GitHub's secondary rate
  limit is generous for read-only `gh issue list` calls; 5–30
  seconds is fine for personal use. Bigger orgs should go higher.

## Reading issue detail in prompts

Stage prompts can render Vik template values directly:

> If `issues.pull.command` returned extra fields, for example `branch`, they
> are available as issue template values such as `{{ issue.branch }}`.

```text
You are working on issue {{ issue.id }}: {{ issue.title }}.
State: {{ issue.state }}
Workdir: {{ issue.workdir }}
```

But the pull command only carries the small subset of fields you
asked for. Anything richer — body, comments, attached PRs, reviewers
— must be fetched fresh inside the prompt itself, because the issue
may have moved by the time the agent runs.

### Fetch the full issue at the start of a stage

```sh
gh issue view {{ issue.id }} \
  --json number,title,body,state,labels,assignees,comments,url,updatedAt
```

Useful JSON keys you can ask for:

- `number`, `title`, `body`, `url`
- `state` (`OPEN`/`CLOSED`), `labels`, `assignees`, `milestone`
- `comments` — full comment thread, with `body`, `author`,
  `createdAt`.
- `closingIssuesReferences` — issues this issue closes.
- `linkedBranches` — branches GitHub auto-linked.
- `projectItems` — project board entries.

Any field listed in `gh issue view --help` works.

### Fetch attached pull requests

```sh
gh pr list --search "linked:{{ issue.id }} repo:owner/name" \
  --state all --json number,title,state,isDraft,url,headRefName
```

Or, when your prompt opens a PR with `Closes #{{ issue.id }}`:

```sh
gh pr view <pr-number> \
  --json number,title,state,isDraft,reviews,statusCheckRollup,mergeable,url
```

### Read review comments and CI state

```sh
gh pr view <pr-number> --json reviews,reviewDecision
gh pr checks <pr-number>
```

For inline review comments specifically (the line-by-line ones), use
the API directly:

```sh
gh api repos/owner/name/pulls/<pr-number>/comments
```

## Managing state from prompts

Vik never updates GitHub. Your prompt sources must include the exact
commands the agent should run when it wants to move the issue
forward. Pick the same convention you used in the pull command.

### Label-based state transitions

```sh
gh issue edit {{ issue.id }} --remove-label todo --add-label work
gh issue edit {{ issue.id }} --remove-label work --add-label review
```

Always _remove_ the previous state label and _add_ the new one in a
single call. Otherwise the issue may briefly carry both, and the
pull command's "exactly one state label" filter will skip it.

### Project-field state transitions

```sh
gh project item-edit \
  --id <item-id> \
  --field-id <status-field-id> \
  --project-id <project-id> \
  --single-select-option-id <option-id>
```

The IDs are stable per project; cache them in environment variables
or as a small helper script that the prompt can call.

### Closing the issue

The cleanest way is to let GitHub close it through a PR closing
keyword. In the prompt, instruct the agent to add `Closes
#{{ issue.id }}` to the PR body. When the PR merges, the issue
auto-closes. No explicit `gh issue close` needed.

Manual close, when you need it:

```sh
gh issue close {{ issue.id }} --reason completed
```

## Common prompt operations

### Leave a comment

```sh
gh issue comment {{ issue.id }} --body "Plan posted; moving to work."
```

For multi-line bodies, write to a temp file first:

```sh
cat > /tmp/comment.md <<'EOF'
## Plan

1. Step one
2. Step two
EOF
gh issue comment {{ issue.id }} --body-file /tmp/comment.md
```

### Update an existing comment

`gh` does not have a one-line edit, but the API does:

```sh
gh api -X PATCH repos/owner/name/issues/comments/<comment-id> \
  -f body="$(cat /tmp/updated.md)"
```

### Open and link a PR

```sh
git push -u origin HEAD
gh pr create \
  --title "<short title>" \
  --body "Closes #{{ issue.id }}

  ...details..." \
  --label vik
```

### Avoid pull request hits in issue searches

`gh issue list` already excludes PRs. If you use `gh search issues`,
add `is:issue` to the query so PR results do not leak in.

## Sanity checks before you run Vik

```sh
# 1. Pull command prints a JSON array, not an error or empty string.
gh issue list --label vik --state open --json number,title,labels \
  | jq 'length'

# 2. Auth works for issue + PR write.
gh issue edit <test-issue> --add-label vik && \
  gh issue edit <test-issue> --remove-label vik

# 3. Vik schema is happy.
vik doctor ./workflow.yml
```

## Related

- [Get Started](../get-started.md)
- [Configuration](../configuration.md)
- [Linear Issue Source](linear.md)
- [Feishu Base Issue Source](feishu.md)
