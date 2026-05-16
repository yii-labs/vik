# Linear Issue Source

Vik does not talk to Linear on its own. Listing issues, reading
detail, leaving comments, moving state — every tracker action is
something *you* tell Vik to do through `issues.pull.command` or
through commands written into your stage prompt sources.

This guide assumes you have read [Get Started](../get-started.md) and
have a working `workflow.yml`. There are two integration surfaces and
they are very different:

- **Pull command** — Vik runs this on a loop, on its own. It must be
  a plain shell command. We use Linear's GraphQL API directly here.
- **Prompt operations** — anything the agent does once it is running
  (read description, post a comment, move state, add a label). The
  agent has tools, so we strongly recommend pointing it at the
  [Linear MCP server](https://linear.app/docs/mcp) and letting it
  call typed tools instead of writing GraphQL by hand.

## Credentials

Linear uses a personal API key. Generate one in
**Linear → Settings → API → Personal API keys** and export it where
Vik can see it:

```sh
export LINEAR_API_KEY="lin_api_..."
```

For detached daemons (`vik run -d`), put the export in whichever
environment file your shell loads at start, or in a wrapper script
you launch Vik from. Vik does not load `.env` files.

> Never `echo`, `cat`, or commit the key. If you store it in a shell
> file, make sure the file is git-ignored.

`vik doctor` only checks `workflow.yml`. It does not call Linear —
your pull command and prompts must fail loudly when auth is broken.

## Designing the issue pull command

`issues.pull.command` is the one spot where MCP cannot help: Vik runs
this command itself, not through an agent, so it must be a plain
shell snippet. We use the GraphQL API directly here.

It must print one JSON array of issue objects to stdout. Each issue
must include at least:

- `id` — Linear issue identifier like `ENG-123`. Vik uses this for
  workspace folder names, so prefer the human-readable identifier
  over the internal UUID.
- `title` — issue title.
- `state` — the value Vik will match against
  `issue.stages.<stage>.when.state`. Case-sensitive.

Linear has a real workflow state field, so the typical pattern is:

> Pull issues assigned to a particular team / cycle / view, project
> them onto the workflow state name, and emit one row per issue.

### Pattern: query issues by team and state

Save this as `./scripts/linear-issues.sh` and `chmod +x` it:

```sh
#!/usr/bin/env bash
set -euo pipefail

: "${LINEAR_API_KEY:?LINEAR_API_KEY is required}"

# Adjust the where: clause to match your workflow.
QUERY='
query {
  issues(
    filter: {
      team: { key: { eq: "ENG" } }
      state: { type: { in: ["unstarted", "started", "review"] } }
    }
    first: 50
    orderBy: createdAt
  ) {
    nodes {
      identifier
      title
      state { name }
    }
  }
}'

curl -sS https://api.linear.app/graphql \
  -H "Authorization: $LINEAR_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$(jq -n --arg q "$QUERY" '{query: $q}')" \
| jq '
    [
      .data.issues.nodes[]
      | { id: .identifier, title: .title, state: .state.name }
    ]
  '
```

Then in `workflow.yml`:

```yaml
issues:
  pull:
    command: ./scripts/linear-issues.sh
    idle_sec: 10
```

What the script does:

1. Refuses to run if `LINEAR_API_KEY` is missing.
2. Queries the GraphQL API for issues in the `ENG` team that are in
   one of the active workflow categories.
3. Reshapes each issue into Vik's `{id, title, state}` shape, using
   the **state name** (e.g. `Todo`, `In Progress`, `In Review`) as
   `state`. Match this exact string in your `workflow.yml` stages.

### Pattern: query a saved view

If you already have a Linear "view" that gathers the right issues,
query it instead — that way the URL bar in Linear is the source of
truth:

```sh
QUERY='
query ($viewId: String!) {
  customView(id: $viewId) {
    issues(first: 50, orderBy: updatedAt) {
      nodes { identifier title state { name } }
    }
  }
}'
```

Pass `$viewId` as a GraphQL variable. Find the view ID in the view's
URL.

### Tips for any pull command

- **Quote `state` exactly** as Linear shows it in the UI. Linear
  state names are case-sensitive *and* may contain spaces (`In
  Progress`). If your workflow state names contain spaces, write the
  matching `when.state:` value the same way.
- **Stay under the rate limit.** Linear's default API allowance is
  generous, but `idle_sec: 5` against a busy workspace can still be
  noisy. 10–30 seconds is a healthier default for production.
- **Test the command by hand.** Run it in a shell and verify the
  output is a JSON array. The Vik intake fails the cycle if stdout
  is not parseable JSON.
- **Pin the team / cycle.** Without a filter you will pull every
  issue you have access to.

## Inside prompts: use the Linear MCP server

Once a stage starts, the agent (Codex or Claude Code) is the one
talking to Linear. Both runtimes support
[MCP](https://modelcontextprotocol.io), and Linear publishes a first-
party MCP server with typed tools for everything below: `get_issue`,
`update_issue`, `create_comment`, `update_comment`, `list_comments`,
`add_label`, `create_attachment`, etc.

Compared to writing GraphQL inside prompts, MCP gives you:

- **Less prompt code.** The agent calls one named tool instead of
  serializing GraphQL into a heredoc.
- **Fewer secrets in flight.** The MCP server holds the auth; the
  prompt does not need to mention `LINEAR_API_KEY`.
- **Schema validation.** Wrong arguments fail at the tool call, not
  three layers deep in a `curl` pipeline.
- **Better agent behavior.** Tool calls land in the agent's training
  distribution; raw `curl ... graphql` is a foreign sequence the
  agent has to assemble.

If you can use MCP, use it. The GraphQL fallback at the end of this
guide is for environments where MCP is not an option.

### Install and connect Linear MCP

Follow Linear's setup page: <https://linear.app/docs/mcp>. The short
version:

1. Open Linear → **Settings → API → MCP** and start the OAuth flow.
2. Configure your agent runtime to launch the Linear MCP server.
   - **Claude Code**: `claude mcp add linear` and follow the prompts,
     or edit your MCP config to add the Linear server entry.
   - **Codex**: add the Linear MCP entry under `mcp_servers` in
     `~/.codex/config.toml`.
3. Restart Vik so the next stage session inherits the new MCP
   configuration.

Confirm the agent sees the tools by running `claude mcp list` (or
the Codex equivalent) before launching Vik.

### Tell prompts to use the MCP tools

Stage prompts can render Vik template values directly:

```text
You are working on Linear issue {{ issue.id }}: {{ issue.title }}.
State: {{ issue.state }}
Workdir: {{ issue.workdir }}
```

> If your pull command returned extra fields, for example `priority`, they are
> available as issue template values such as `{{ issue.priority }}`.

For everything richer, instruct the agent to call the MCP tool by
name. Example fragment from a `plan.md` prompt:

```md
## Read the issue

Use the Linear MCP `get_issue` tool with `id: "{{ issue.id }}"` and
fetch:

- description
- state
- priority
- labels
- comments (oldest first)
- attachments (look for GitHub PRs in the URL)

Treat the MCP response as ground truth. Do not call the Linear API
directly.
```

### Common prompt operations (MCP)

Phrase prompts around the tool names; the agent picks the arguments.

- **Read full detail** — `get_issue { id: "ENG-123" }`. Returns
  description, state, comments, labels, attachments, parent /
  children, etc.
- **List comments** — `list_comments { issueId: "ENG-123" }` for
  paginated reads when you only need the comment thread.
- **Post a comment** — `create_comment { issueId: "ENG-123",
  body: "..." }`. For multi-line bodies, instruct the agent to write
  the markdown into the body argument directly.
- **Edit a comment** — `update_comment { id: "<comment-uuid>",
  body: "..." }`. The UUID comes from a previous `create_comment`
  result or from `get_issue.comments`.
- **Move state** — `update_issue { id: "ENG-123", stateId:
  "<state-uuid>" }`. For convenience, ask the agent to first call
  `get_workflow_states { teamId: "..." }` and pick the UUID by name
  — no need to maintain a hand-curated UUID table.
- **Add or replace labels** — `update_issue { id, labelIds: [...] }`.
  `labelIds` overwrites; have the agent read existing
  `labels.nodes[].id` first when it should add rather than replace.
- **Attach a PR** — `create_attachment { issueId, url, title }`.
  Most teams rely on Linear's GitHub integration to do this
  automatically when the branch name or PR body contains the issue
  identifier; manual attachment is the fallback.

### Closing the issue

Move it to a state with `type: completed` (typically named `Done`).
Linear has no separate "close" verb.

In a prompt:

```md
Use the Linear MCP `update_issue` tool to set the issue state to the
team's `Done` state. Look up the state UUID with
`get_workflow_states { teamId: "<team>" }` if you do not already
have it.
```

For canceled work, use a `canceled`-typed state instead.

## Fallback: prompt operations without MCP

When the runtime cannot run MCP, fall back to GraphQL the same way
the pull command does. Below is the minimum needed to mirror the
operations above. Prefer the MCP path whenever possible — these
snippets exist for completeness only.

### Read full issue detail

```sh
#!/usr/bin/env bash
# linear-issue-view <identifier>
set -euo pipefail
: "${LINEAR_API_KEY:?}"
ID="$1"

QUERY='
query ($id: String!) {
  issue(id: $id) {
    identifier title description url
    state { name type }
    priority
    labels { nodes { id name } }
    parent { identifier title }
    children { nodes { identifier title state { name } } }
    comments(first: 100) {
      nodes { id body user { displayName } createdAt updatedAt }
    }
    attachments { nodes { title url } }
  }
}'

curl -sS https://api.linear.app/graphql \
  -H "Authorization: $LINEAR_API_KEY" \
  -H "Content-Type: application/json" \
  -d "$(jq -n --arg q "$QUERY" --arg id "$ID" '{query: $q, variables: {id: $id}}')"
```

### Move state, leave a comment, etc.

The same `issueUpdate`, `commentCreate`, `commentUpdate`, and
`attachmentCreate` mutations from earlier versions of this doc still
work. The shape:

```sh
MUT='
mutation ($id: String!, $stateId: String!) {
  issueUpdate(id: $id, input: { stateId: $stateId }) { success }
}'
curl -sS https://api.linear.app/graphql ... -d "$(jq -n --arg q "$MUT" --arg id "$ID" --arg stateId "$SID" \
        '{query: $q, variables: {id: $id, stateId: $stateId}}')"
```

State changes through `issueUpdate` need the **state UUID**, not the
state name. Cache the UUIDs once per workspace by querying
`team(id: ...) { states { nodes { id name type } } }`.

## Sanity checks before you run Vik

```sh
# 1. Linear API key works.
curl -sS https://api.linear.app/graphql \
  -H "Authorization: $LINEAR_API_KEY" \
  -H "Content-Type: application/json" \
  -d '{"query":"{ viewer { id name } }"}' | jq

# 2. Pull command prints a JSON array.
./scripts/linear-issues.sh | jq 'length'

# 3. (If using MCP) the agent can see the Linear tools.
claude mcp list   # or the Codex equivalent

# 4. Vik schema is happy.
vik doctor ./workflow.yml
```

## Related

- [Get Started](../get-started.md)
- [Configuration](../configuration.md)
- [GitHub Issue Source](github.md)
- [Feishu Base Issue Source](feishu.md)
- [Linear MCP server](https://linear.app/docs/mcp)
