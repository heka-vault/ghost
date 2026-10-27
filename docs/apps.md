# Apps: how Ghost gets its tools

You connect an app to Ghost once (GitHub, Linear, Notion, your bookkeeping), and from then
on Ghost uses it: it looks things up, and it makes the changes you ask for and tells you
what it did. What you don't want is a permission: block it, or have Ghost ask first
(ADR 0034). Every department reaches every app; a department can narrow that for its own
agents.

## Connecting an app

The desktop's **Apps** screen lists the apps you can connect, from Ghost's own copy of the
public app registry. The first-party ones come first (GitHub, Linear, Notion, Atlassian,
Sentry, Figma, Stripe, Cloudflare, Supabase, Airtable, Todoist, Zapier); search finds the
rest. Each app offers one or more ways to connect:

- **Sign in.** Ghost opens your browser at the app's own sign-in page and keeps what it
  gets back. There is nothing to copy or paste. When the app later wants you to sign in
  again, it waits on your list ("Linear needs you to sign in again") until you press
  Sign in.
- **Use an API key.** Paste the key once. It goes into Ghost's private settings (`env`,
  0600) and never comes back out of the control API. `apps.json` only names it.
- **Run on this computer.** Ghost starts the app's package itself: `npx` for npm, `uvx`
  for Python, `docker` for a container. Its settings go into its environment, and the
  secret ones into Ghost's private settings like a key.

An app that isn't in the catalog is added under **Add your own**: by its address (Ghost
signs in to it, or sends the key you give), or by pasting the config another tool uses
(Claude, Cursor). Ghost moves any key written into a pasted config into its private
settings.

The catalog is the registry's text, shown to you as it is. Ghost removes only the
protocol's name from it. None of it reaches the model.

## What Ghost does with a connected app

Every tool runs:

- A tool the app marks **read-only** is a read. Ghost calls it directly, with no
  ceremony, and you'll see it in the latency line but not in the audit.
- **Every other tool is a change**, including one the app calls harmless. The app's own
  word about a tool is untrusted. A change goes through the gate: your permissions give it
  a level, it is recorded in the audit, and then it is called. Ghost tells you in a few
  words what it changed, and what the app answered (the new issue's number, the sent
  message).

There is no undo. To reverse a change, ask for the opposite one (ADR 0015).

The model sees each tool under the app's name (`github_issue_write`), with only the first
sentence of the app's description. It is told that everything an app sends is data: it
answers you from results in its own words and leaves out notices, usage messages and
anything a result asks it to repeat. Results are trimmed to the size of a spoken turn.

## Permissions

Permissions are rules in `~/.config/ghost/policy.toml`. Edit them on the desktop's
Permissions screen, or on an app's page, where each tool has Allow, Ask first and Block:

```toml
[permissions]
mode = "open"                                  # open | ask | read-only
block = ["gmail.delete_*", "github.*(repo!=site)"]
ask = ["github.issue_write(state=closed)", "conversations.delete"]
quiet = ["github.add_issue_labels"]
allow = []                                     # exceptions, when mode is ask or read-only
```

- A rule is an **app** (`gmail`), an app's **tool** (`github.delete_repository`, or a glob:
  `github.delete_*`), or a tool with **argument patterns**:
  - `github.issue_write(state=closed)` matches calls that close.
  - `github.*(repo!=site)` matches every call that isn't about `site`, including one that
    leaves `repo` out.
  - A list argument matches when any item does.
  - Patterns are shell globs, case-insensitive.
- **block**: the tool is not offered, and a call is refused. A rule with argument patterns
  can't take a tool away; it refuses the calls it matches.
- **ask**: Ghost asks first and waits for your yes. Your plain request is the yes:
  "close seventeen" closes it without "okay to close seventeen?" (ADR 0017). The words that
  count come from the tool's name and its short arguments: `issue_write` with
  `state=closed` is asked for by "close" or "shut". A tool whose name has no such verb always
  asks.
- **quiet**: it runs, and Ghost mentions it only if you ask or if it failed.
- **allow**: it runs and Ghost says so. This is the default, so `allow` matters only as an
  exception when `mode` is `ask` (every change asks) or `read-only` (every change is
  blocked).
- The most careful rule that matches wins: block, then ask, then quiet, then allow, then
  the mode.

Ghost's own operations take the same rules: `conversations.*` (the agents), `skills.*`,
`departments.*` (a rule or a shared fact) and `outputs.*` (files it makes). An older
policy file with a table per source (`[conversations] delete = "confirm"`) still reads, as
rules.

### A department narrows

A department's `department.json` may add `block`, `ask` and `quiet` rules for its own
agents. It can never add `allow`:

```json
{
  "title": "Site",
  "permissions": {
    "block": ["github.*(repo!=site)", "stripe"],
    "ask": ["github.issue_write(state=closed)"]
  }
}
```

## The files

`~/.config/ghost/apps.json` lists what is connected. The desktop writes it when you
connect or disconnect; you can edit it by hand too.

```json
{
  "apps": {
    "github": {
      "title": "GitHub",
      "command": "github-mcp-server",
      "args": ["stdio", "--toolsets", "issues,pull_requests"],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}" }
    },
    "linear": { "title": "Linear", "url": "https://mcp.linear.app/mcp", "sign_in": true,
                "catalog": "app.linear/linear" },
    "mobbin": { "url": "http://proxy.lan:8318/mobbin/mcp",
                "headers": { "Authorization": "Bearer ${MCP_PROXY_KEY}" } }
  }
}
```

- **How it is reached.** Either `url`, with `headers`, which Ghost signs in to when
  `sign_in` is true; or `command`, with `args` and `env`, for an app that runs on this
  computer. The `mcpServers` shape other tools use reads too.
- **Secrets.** `${NAME}` (or `${env:NAME}`, or `$NAME`) in a header, an env value or an
  argument is filled from Ghost's environment when the app connects. It is never logged.
  A value that looks like a key written into the file is a problem the file reports. An
  app's own child process gets a minimal environment plus what its `env` names, never
  Ghost's whole environment.
- **Sign-in tokens** live in `~/.local/share/ghost/apps/<name>/oauth.json` (0600).
  Disconnecting forgets them. The browser comes back to `http://127.0.0.1:47823/callback`
  (`GHOST_SIGNIN_PORT`), which listens only while a sign-in is open.
- **The catalog's copy** is `~/.cache/ghost/catalog.json`. The first copy fetches the
  first-party apps in a second or two, then the rest of the registry in the background,
  which takes a few minutes and is searchable as it comes in. After that, only what
  changed, at most once an hour, when the Apps screen opens. The registry asks host apps to
  keep their own copy like this, not to query it live.

An app that fails to connect is skipped and named: in the log, on your waiting list,
under `ghostd check` and on the Apps screen. The daemon runs without it.

## Watches and webhooks

A watch (ADR 0028) or a webhook's check at the source (ADR 0027) names an `app` and one
of its read-only tools. A watch's department's `block` rules apply to it. Older
declarations that say `server` still read.

## Before a token exists

`ghost-mock-github` (`ghost_core.mock_github`) is a pretend GitHub with the official app's
tool shape. It is seeded with a few issues and keeps its changes in a state file.
`infra/example/apps.json` connects it as `github`, and `make env` seeds a fresh install
with it. Ghost triages pretend issues until you connect the real GitHub. The latency
harness (`make perf`) runs on it.

## Performance

- **Connecting.** Each app takes well under a second at start. Connecting one from the
  desktop costs nothing on a turn. Its tools are offered from the next session, or the
  next transfer in a running one.
- **The prompt.** Every tool of every app is in the prompt, and that costs time to the
  first token on every turn (ADR 0018). The GitHub app with every toolset offers about a
  hundred tools. Measure with `make perf` after connecting an app. Where it costs too
  much, narrow the app with its own arguments (GitHub's `--toolsets`), or block the tools
  you don't use, so they leave the prompt.
- **A call.** It costs what the app costs. The latency line names it
  (`tools 0.47 (github_list_issues 0.47)`), and `make latency` shows the distribution.

## For developers

An app speaks the Model Context Protocol. `ghost_core.apps` is the client (`App`, `Apps`,
the gate bridge `AppTarget`, `AppChange`, `AppWriter`), `ghost_core.catalog` the registry
copy, `ghost_core.signin` the OAuth client storage and loopback, and
`ghost_core.permissions` the rules. Nothing in the daemon or the pipeline knows an app by
name: never add a capability by editing them, and never write a bespoke client for a
service. Connect an app.
