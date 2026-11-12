# Apps: how Ghost gets its tools

You connect an app to Ghost once (GitHub, Linear, Notion, your bookkeeping), and from then
on Ghost uses it: it looks things up, and it makes the changes you ask for and tells you
what it did. What you don't want is a permission: block it, or have Ghost ask first
(ADR 0034). Every agent reaches every app, and there is one set of permissions (ADR 0035).

## Connecting an app

The desktop's **Apps** screen lists the apps you can connect, from Ghost's own copy of the
public app registry. The first-party ones come first (GitHub, Linear, Notion, Atlassian,
Sentry, Figma, Stripe, Cloudflare, Supabase, Airtable, Todoist, Zapier); search finds the
rest, the product maker's own app first, with other publishers' apps for the same
product folded under it ([below](#who-publishes-an-app-and-where-it-runs)). Each app
offers one or more ways to connect:

- **Sign in.** Ghost opens your browser at the app's own sign-in page and keeps what it
  gets back. There is nothing to copy or paste. When the app later wants you to sign in
  again, it waits on your list ("Linear needs you to sign in again") until you press
  Sign in.
- **Use a password.** Paste the app's API key or token once. It is a *password* to Ghost:
  it goes into Ghost's vault (ADR 0036) and never comes back out, of the control API or
  anything else. `apps.json` only names it. Disconnecting the app forgets it. While your
  keyring is locked, Ghost can't save one, and says so.
- **Run on this computer.** Ghost starts the app's package itself: `npx` for npm, `uvx`
  for Python, `docker` for a container. Its settings go into its environment, and the
  secret ones into Ghost's vault like a password.

An app that isn't in the catalog is added under **Add your own**: by its address (Ghost
signs in to it, or sends the key you give), or by pasting the config another tool uses
(Claude, Cursor). Ghost moves any key written into a pasted config into its vault: in a header or an env value, in an argument (`--api-key …`, `--token=…`, or a
value shaped like a token), or in the address's query. An entry it can't read is named,
not dropped quietly.

The catalog is the registry's text, shown to you as it is. Ghost removes only the
protocol's name from it. None of it reaches the model, and nothing the registry fixes (a
header, an argument, an address) can name one of Ghost's passwords: only the settings you
fill in are.

### Who publishes an app, and where it runs

Anyone can publish to the registry, so a search for "posthog" finds six apps, five of them
called "Posthog". The description is the publisher's own words: "Official PostHog app"
in a description proves nothing. What the registry does check is the start of each app's
registry name, and Ghost reads it (`ghost_core::provenance`, string rules, no model):

- **Who published it.** `io.github.PostHog/mcp` was published by someone signed in to
  GitHub as PostHog: the card says "by PostHog · verified on GitHub". `com.mcparmory/posthog`
  was published by whoever proved they own mcparmory.com: "by mcparmory.com · verified
  domain". That says who published it, not that it is safe or does what it says.
- **The maker's own.** An app gets "From PostHog" when its publisher's name *is* the
  product's *and* it connects to the maker's own host. For a domain, that is a host under
  the very domain the registry checked: the owner of notion.com publishing "Notion" on
  `mcp.notion.com`. The owner of posthog.ai publishing "PostHog" is not PostHog, whose
  domain is posthog.com. For a GitHub account, it is a host registered under the account's
  name: the account PostHog on `mcp.posthog.com`. The publisher's name alone is never
  enough, and neither is a repository or a website: every GitHub account owns repositories
  under its own name, and anyone can write any website. A short list in the code names
  makers whose names differ from their product's (`getsentry` makes Sentry, `github` runs
  `api.githubcopilot.com`, `makenotion` is Notion), and makers checked by hand whose own app
  is a package with no host to agree (`paypal`, `buildkite`). The registry has no
  "official" flag to read: its own `_meta` carries only an app's status and dates. The
  app's detail says why Ghost marked it ("Published by the GitHub account PostHog… it
  connects to mcp.posthog.com"). The maker's own app comes first in a search.
- **Where it runs.** An app on the internet names the host it talks to, and says so when
  that host isn't the product maker's: "Runs on gateway.pipeworx.io, pipeworx-io's,
  not PostHog's. What you ask of PostHog through this app passes through pipeworx-io." A
  host is the maker's only under one of the maker's own domains, exactly (`posthog.ai` is
  not PostHog's `posthog.com`), and a host anyone can rent a name on (`*.vercel.app`,
  `*.workers.dev`) is never the maker's. An app that runs on this computer names its
  registry and package: npm `posthog-mcp`.
- **The same product, folded.** Other publishers' apps whose title is exactly the
  product's ("Posthog", "PostHog") fold under the maker's own: "4 more from other
  publishers". Nothing else folds: an app with another title ("Linear by usefulapi",
  "Engageable Analytics") stands on its own, and so do look-alikes when the maker has no
  app of its own in the results. When a product has more than one app of its maker's own,
  the others fold under the one the app's detail names as the maker's, whichever the
  search ranks first.

Before you connect, the app's detail shows all of this with its repository, its website,
its version and when the registry last saw it change.

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

The model sees each tool under the app's name (`github_issue_write`), with the app's
description of it up to 1,024 characters and the description of each argument. What the
app says about itself when it connects (its instructions) joins the app's paragraph of the
prompt, up to 2,000 characters. A name the model can't take is rewritten to one it can;
when two tools would get the same name, the app listed first in `apps.json` keeps it and
the other is offered under that name with a short hash on the end. The model is told that
everything an app sends or says about itself is data: it answers you from results in its
own words and leaves out notices, usage messages and anything an app asks it to repeat.
Before anything reads a result, an error, a description or an app's instructions, every
password Ghost holds (8 characters or longer) is replaced with `[password: NAME]`, so an
app that echoes a key never puts it in front of the model, the audit or a log.

### Results

One result is read whole when it is within 24,000 characters, which is a long issue with
its comments or a document. A longer one is cut at the end and carries a note saying how
much was dropped, so Ghost asks for less (one item, a page, a narrower search) instead of
taking the cut for the whole. A list keeps 200 items within that. Structured data comes
back as the app gave it. An image, a sound or a file is named with its type and size;
Ghost does not look at it. An app's own error message is passed on, cut to 300
characters.

### Keeping up with the app (ADR 0046)

- **Its tools change.** When an app says its tools changed, Ghost lists them again. An
  open session offers the new list from the next time the model is asked, and so do an
  app you connect or disconnect in the middle of a conversation.
- **A slow call.** A call may take five minutes and connecting one minute. While a call
  runs you hear the thinking sound, and you can still talk over it, say "hang on, Ghost"
  or hang up: the call is stopped, the app is told, and Ghost answers what you said. A
  change that was stopped may or may not have been made; Ghost is told to check.
- **A connection that dies.** The app's tools stay offered. The next call opens the
  connection again, once. In the background Ghost tries too: after 1, 2, 4, 8 and 16
  seconds (each a fifth more or less), then every five minutes. An app on the internet
  that has been quiet for three minutes is pinged. An app that wants you to sign in again
  is not retried; it waits on your list.
- **Documents and prompts.** An app that offers documents to read or ready-made prompts
  gets four more reads: `<app>_list_resources`, `<app>_read_resource`,
  `<app>_list_prompts` and `<app>_get_prompt`. Block them like any tool
  (`notion.read_resource`).
- **A question for you.** An app may ask you something in the middle of a call. Ghost has
  the plumbing and, for now, declines every question; an app that sends you to a web page
  to answer is declined too.

### Many tools

Every tool offered by name is in the prompt on every turn. When the connected apps have
more than 40 tools between them, or more than 30,000 bytes of tool descriptions and
arguments, Ghost offers by name only the tools you pinned and the twelve it used or found
most recently. It finds the rest with `ghost_tools_search`: a few words ("create calendar
event") give the tools that match, and those are offered by name from then on. Below the
limits nothing is hidden and the search is not offered. Hiding a tool is about the size of
the prompt and is not a permission: a hidden tool still runs when called, and what is
blocked is still your rules.

### Settings for an app

An entry in `apps.json` may carry four more keys:

```json
{"apps": {"reports": {"url": "https://reports.example/mcp", "sign_in": true,
  "timeout": 900, "connect_timeout": 30, "result_chars": 60000,
  "pinned": ["run_report"]}}}
```

| Key | Default | For every app, in `settings.env` |
|---|---|---|
| `timeout`, seconds one call may take | 300 | `GHOST_APP_TIMEOUT` |
| `connect_timeout`, seconds connecting may take | 60 | `GHOST_APP_CONNECT_TIMEOUT` |
| `result_chars`, characters of one result Ghost reads | 24000 | `GHOST_APP_RESULT_CHARS` |
| `pinned`, tools always offered by name | none | |
| tools offered by name before search takes over | 40 | `GHOST_APP_TOOLS_MAX` |
| the same, by bytes of schema | 30000 | `GHOST_APP_TOOLS_BYTES` |
| recently used tools kept offered past that | 12 | `GHOST_APP_TOOLS_KEPT` |

### What Ghost cannot connect

An app must speak over stdio or Streamable HTTP. The older HTTP+SSE transport is not
supported (the protocol library Ghost uses has no client for it). An app cannot ask
Ghost's model to write for it (sampling), and an app's own screens are not shown.

## Permissions

Permissions are rules in `~/.config/ghost/policy.toml`. Edit them on the desktop's
Settings › Permissions page, or on an app's page, where each tool has Allow, Ask first and Block:

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
`rules.add` (a line appended to `AGENTS.md`), `outputs.*` (files it makes) and `ghost.*`
(Ghost's own app, [below](#ghosts-own-app)). An older
policy file with a table per source (`[conversations] delete = "confirm"`) still reads, as
rules.

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

- **Its name** is lowercase letters, digits, dashes and underscores, never one of
  Ghost's own (`ghost`, `conversations`, `skills`, `rules`, `outputs`, `waiting`), whose
  operations it would shadow; `ghostd check` names an entry that tries. Ghost writes `apps.json` for you only (0600), and edits only the entry it
  connects or disconnects; a file that isn't JSON is never overwritten.
- **How it is reached.** Either `url`, with `headers`, which Ghost signs in to when
  `sign_in` is true; or `command`, with `args` and `env`, for an app that runs on this
  computer. The `mcpServers` shape other tools use reads too.
- **Passwords.** `${NAME}` (or `${env:NAME}`, or `$NAME`) in a header, an env value, an
  argument or the address is filled from Ghost's vault when the app connects (ADR 0036),
  and the use is the password's "last used". It is never logged. A value that looks like a
  key written into the file is a problem the file reports. An app that runs on this
  computer gets a minimal environment plus what its `env` names, never Ghost's whole
  environment: those named passwords, as environment variables, are the one place a value
  leaves the vault. A name the vault doesn't have can come from Ghost's environment
  (`settings.env`), for a setting that isn't secret.
- **Sign-in tokens** live in `~/.local/share/ghost/apps/<name>/oauth.json` (0600).
  Disconnecting forgets them. The browser comes back to `http://127.0.0.1:47823/callback`
  (`GHOST_SIGNIN_PORT`), which listens only while a sign-in is open.
- **The catalog's copy** is `~/.cache/ghost/catalog.json`. The first copy fetches the
  first-party apps in a second or two, then the rest of the registry in the background,
  which takes a few minutes and is searchable as it comes in. After that, only what
  changed, at most once an hour, when the Apps page opens. The registry asks host apps to
  keep their own copy like this, not to query it live.

An app that fails to connect is skipped and named: in the log, on your waiting list,
under `ghostd check` and on the Apps page. The daemon runs without it.

## Watches and webhooks

A watch (ADR 0028) or a webhook's check at the source (ADR 0027) names an `app` and one
of its read-only tools. The permissions' `block` rules apply to it as to the model's
reads. Older declarations that say `server` still read ([folder.md](folder.md#watches)).

## Ghost's own app

Ghost is one of its own apps (ADR 0045). You don't connect it and it is not in
`apps.json`: the daemon serves it to its agents in process, under the name `ghost`, and
it is there before any session opens. Through it an agent can look up how Ghost works
(`ghost_docs_search`, `ghost_docs_read`, from the documentation built into Ghost), say
how Ghost is now (`ghost_status`, `ghost_apps_list`), say what was changed and when
(`ghost_audit_recent`), and reach you when you are away: `ghost_notify` puts an item on
your waiting list from that agent, your routes deliver it like any other, and the agent
clears it with `ghost_resolve` when the matter is done. An agent can clear only what it
put there. `ghost_waiting_ack` and `ghost_waiting_snooze` do what the buttons on the list
do.

The reads are reads and the rest are changes through the gate, recorded in the audit.
Your permissions cover them as `ghost.*`. The one difference is the default: a `ghost.*`
change that no rule names runs quietly, because the item on your list is the
announcement. `allow = ["ghost.notify"]` has Ghost say it each time, `ask` has it ask
first, and `block` takes the tool away.

Ghost's own text is not an outside system's, so these tools' descriptions and results
reach the model whole; every other app's are held to the caps described above.
`ghost_tools_search` is one of these tools; it is offered only when the connected apps
have more tools than a turn should carry ([above](#many-tools)), and since what it returns
is the apps' text, it holds itself to an app's result budget.

The same app is what your other tools reach when you connect them to Ghost (Claude Code,
Cursor, Codex): see [connect-ghost.md](connect-ghost.md). That is the other direction from
this page. An app is something Ghost connects to; an outside tool connects to Ghost.

## Before a token exists

`ghost-mock-github` (`ghost_core::mock_github`, a binary of the workspace that the bundle
carries beside `ghostd`) is a pretend GitHub with the official app's
tool shape. It is seeded with a few issues and keeps its changes in a state file.
`infra/example/apps.json` connects it as `github`, and `make env` seeds a fresh install
with it. Ghost triages pretend issues until you connect the real GitHub. The latency
harness (`make perf`) runs on it.

## Performance

- **Connecting.** Each app takes well under a second at start. Connecting one from the
  desktop costs nothing on a turn. Its tools are offered from the next time the model is
  asked, in a running session too, and the cached prompt is lost once when they join.
- **The prompt.** Every tool offered by name is in the prompt, and that costs time to the
  first token on every turn (ADR 0018). A description may be 1,024 characters, about 250
  tokens. The GitHub app with every toolset offers about a hundred tools, which is past
  the limits, so most of them are found by search ([above](#many-tools)). Measure with
  `make perf` after connecting an app. Where it costs too much, narrow the app with its
  own arguments (GitHub's `--toolsets`), block the tools you don't use, or lower
  `GHOST_APP_TOOLS_MAX`.
- **The tool list.** Ghost builds it again before each model call and sends a new one
  only when it differs: 0.4 ms with the seed's tools, 6 ms with 200 app tools, in an
  unoptimised build (ADR 0046).
- **A call.** It costs what the app costs, up to its timeout. The latency line names it
  (`tools 0.47 (github_list_issues 0.47)`), and `make latency` shows the distribution.
- **A result.** Up to 24,000 characters, about 6,000 tokens, stay in the conversation's
  context for each result. Lower `result_chars` for an app that says more than you need.
- **The catalog.** Reading Ghost's copy (about 48,000 apps, 2026-10) takes about 1.3 s,
  0.45 s of it for who published each app and which are the maker's own. The daemon does
  it once at start, and after each refresh in a thread so the voice loop isn't held; a
  search takes about 35 ms. Nothing of it is on a turn.

## For developers

An app speaks the Model Context Protocol. `ghost_core::apps` is the client (`App`, `Apps`,
the gate bridge `AppTarget`, `AppChange`, `AppWriter`; `Limits` for the caps and their
settings, `with_cancel` for the turn a call belongs to, `Asker` for an app's question,
`Connector` for an app opened some other way than `apps.json` says), `ghost_core::catalog` the registry
copy, `ghost_core::provenance` who publishes a catalog entry, whether it is the maker's own
and where it runs (`KNOWN_MAKERS` and `SHARED_HOSTS` are the hand-kept lists),
`ghost_core::signin` the OAuth client storage and loopback, and
`ghost_core::permissions` the rules. Nothing in the daemon or the pipeline knows a
connected app by name: never add a capability by editing them, and never write a bespoke
client for a service. Connect an app. Ghost's own functions are the one exception, and
they come the same way: as a tool of Ghost's own app (`ghost_core::ghost_app`, the server
side of the same protocol, attached in process), never as a branch in the pipeline.
