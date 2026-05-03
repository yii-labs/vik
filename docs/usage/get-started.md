# Get Started

Use this file as an executable checklist for a human operator or AI agent with
shell access. Use browser automation only for login or settings pages. Never
print, log, or commit secret values.

## 1. Workspace

Why: Vik creates one working copy per active Linear issue. The workspace root
must be a directory where Vik may create, mutate, and remove issue directories.

1. Start in the operator directory that contains `WORKFLOW.md`:

   ```sh
   test -f WORKFLOW.md
   pwd
   ```

2. Confirm required runtime commands are available:

   ```sh
   vik --help
   git --version
   gh --version
   codex --version
   jq --version
   ```

3. Create the issue workspace root used by `WORKFLOW.md`:

   ```sh
   mkdir -p "$HOME/code/vik-workspaces"
   ```

4. Confirm the current directory is a Git worktree before Vik starts creating
   issue workspaces:

   ```sh
   git rev-parse --show-toplevel
   ```

## 2. Workflow

Why: `WORKFLOW.md` tells Vik which Linear issues to claim, where to create
workspaces, how to clone the repo, and how to launch Codex.

1. Open `WORKFLOW.md`.
2. Confirm `tracker.project_slug` matches the Linear project slug.
3. Confirm `tracker.active_states` contains every state Vik may claim.
4. Confirm `tracker.terminal_states` contains terminal states that should stop
   tracking and trigger cleanup.
5. Confirm `workspace.root` points to the directory created above.
6. Confirm `hooks.after_create` clones this repo into the empty issue
   workspace.
7. Confirm `codex.command` launches `codex app-server`.

Validate config parsing after connections are configured:

```sh
vik ./WORKFLOW.md --check
```

Start the daemon:

```sh
vik ./WORKFLOW.md
```

Start with the optional observation server:

```sh
vik ./WORKFLOW.md --port 3000
```

## 3. Connections

### Codex

Why: Vik launches `codex app-server` inside each issue workspace. Codex must be
installed and authenticated before the daemon can run agent sessions.

Official links:

- Codex CLI reference:
  <https://developers.openai.com/codex/cli/reference>
- OpenAI API keys:
  <https://platform.openai.com/settings/organization/api-keys>

Steps:

1. Check CLI availability:

   ```sh
   codex --version
   codex app-server --help
   ```

2. Check auth:

   ```sh
   codex login status
   ```

3. If auth is missing and a browser is available, run:

   ```sh
   codex login
   codex login status
   ```

4. If browser auth is unavailable and `OPENAI_API_KEY` is already exported,
   authenticate without printing the key:

   ```sh
   printenv OPENAI_API_KEY | codex login --with-api-key
   codex login status
   ```

5. Stop with a Codex auth blocker if neither browser auth nor an API key is
   available.

### GitHub

Why: Vik workflow hooks clone repositories. Agents also need GitHub access for
branch, push, PR, label, comment, review, and check operations.

Official links:

- GitHub CLI auth manual: <https://cli.github.com/manual/gh_auth>
- GitHub CLI git credential setup:
  <https://cli.github.com/manual/gh_auth_setup-git>
- GitHub personal access tokens:
  <https://docs.github.com/en/authentication/keeping-your-account-and-data-secure/managing-your-personal-access-tokens>
- GitHub SSH setup:
  <https://docs.github.com/en/authentication/connecting-to-github-with-ssh>

Steps:

1. Check GitHub CLI auth:

   ```sh
   gh auth status --active --hostname github.com
   ```

2. If auth is missing and a browser is available, run:

   ```sh
   gh auth login --hostname github.com --git-protocol ssh --web
   gh auth setup-git --hostname github.com
   gh auth status --active --hostname github.com
   ```

3. If using a token, create it at the personal access token link above. Prefer
   fine-grained access to only `yii-labs/vik`. Grant at least:

   - `metadata: read`
   - `contents: write`
   - `pull_requests: write`
   - `issues: write`
   - `actions: read`

4. Store the token in the environment as `GH_TOKEN` or `GITHUB_TOKEN`. Do not
   commit it:

   ```sh
   test -n "${GH_TOKEN:-}${GITHUB_TOKEN:-}"
   gh auth status --active --hostname github.com
   gh auth setup-git --hostname github.com
   ```

5. The default `WORKFLOW.md` clone hook uses SSH:

   ```sh
   git clone --depth 1 git@github.com:yii-labs/vik .
   ```

   Therefore SSH auth must work:

   ```sh
   ssh -T git@github.com || true
   git ls-remote git@github.com:yii-labs/vik HEAD
   ```

6. If token auth works but SSH auth is unavailable, change
   `hooks.after_create` in `WORKFLOW.md` to HTTPS before starting Vik:

   ```yaml
   hooks:
     after_create: |
       git clone --depth 1 https://github.com/yii-labs/vik .
   ```

7. Stop with a GitHub auth blocker only after both CLI/browser and token paths
   fail.

### Linear

Why: Vik reads candidate issues from Linear, updates issue metadata during
workflow execution, and exposes a `linear_graphql` tool to Codex sessions.

Official links:

- Linear GraphQL API: <https://linear.app/developers/graphql>
- Linear API key settings: <https://linear.app/settings/api>

Steps:

1. Check whether a real key is already available:

   ```sh
   linear_key="${LINEAR_API_KEY:-}"
   if [ -z "$linear_key" ] && [ -f .env ]; then
     linear_key="$(sed -n 's/^LINEAR_API_KEY=//p' .env | tail -n 1)"
   fi
   case "$linear_key" in
     lin_api_* ) : ;;
     ""|lin_api_xxx ) echo "missing real Linear API key" >&2; exit 2 ;;
     * ) echo "unexpected Linear API key format" >&2; exit 2 ;;
   esac
   unset linear_key
   ```

2. If no key exists and a browser is available, open the Linear API key settings
   link above. Create a personal API key for this workspace.

3. Paste the key into the current shell without echoing it:

   ```sh
   stty -echo
   printf "Linear API key: " >&2
   IFS= read -r LINEAR_API_KEY
   stty echo
   printf "\n" >&2
   export LINEAR_API_KEY
   ```

4. Store the key in `.env` while preserving any future variables:

   ```sh
   test -f .env || cp .env.example .env
   tmp="$(mktemp)"
   grep -v '^LINEAR_API_KEY=' .env > "$tmp" || true
   printf 'LINEAR_API_KEY=%s\n' "$LINEAR_API_KEY" >> "$tmp"
   mv "$tmp" .env
   chmod 600 .env
   ```

5. Verify the key against Linear without printing it:

   ```sh
   curl -fsS https://api.linear.app/graphql \
     -H "Content-Type: application/json" \
     -H "Authorization: ${LINEAR_API_KEY}" \
     --data '{"query":"query { viewer { id name } }"}' \
     | jq -e '.data.viewer.id'
   ```

6. Confirm the project slug in `WORKFLOW.md`. Use the slug from the Linear
   project URL or keep the repo default when running this Vik project:

   ```sh
   rg -n 'project_slug' WORKFLOW.md
   ```

7. Stop with a Linear auth blocker if no personal API key can be created or
   provided.

## 4. Run

1. Validate workflow config:

   ```sh
   vik ./WORKFLOW.md --check
   ```

2. Start Vik:

   ```sh
   vik ./WORKFLOW.md --port 3000
   ```

3. Inspect state:

   ```sh
   curl -fsS http://127.0.0.1:3000/api/v1/state | jq .
   ```

4. Stop with `Ctrl-C` for foreground runs.

## 5. Related Docs

- [Docker](docker.md)
- [Service Daemon](service-daemon.md)
- [Configuration](configuration.md)
- [Observation](observation.md)
