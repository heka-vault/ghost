# 0034: Apps connect to Ghost; everything runs unless a permission blocks it

- **Status:** accepted (decided 2026-10-27)
- **Date:** 2026-10-27
- **Supersedes:** ADR 0022's write declarations and the reads list; ADR 0024's servers per
  department (`mcp.json`, `use`, the declarations in `department.json`, `root_servers`) and
  "a session in a department reaches only what its folder gives" for outside tools; ADR
  0019's allowlists and ownership routing
- **Amends:** ADR 0011 (what the levels default to, and where the operator sets them),
  ADR 0017 (where `asked_by` comes from)

Ghost was particular about what it may do: a server's tool reached the model only once the
operator had written a declaration for it, and most changes asked first. In practice the
operator's dictated words reach Ghost about as faithfully as typing, so the asking was
ceremony, and every new service meant a page of JSON before it did anything. The operator
also does not think in MCP servers. He connects GitHub to Ghost the way he signs in to a site
with Google. The operator's decision is to invert the model. Ghost uses every tool it has.
Permissions are a separate thing: a list of what the operator blocks or wants asked first.
The product's word for a connected service is **app**.

How other agents do it (reviewed 2026-10-27, `.scratch/permissions-research.md`): three
levels per tool (Claude's connectors: Always allow, Needs approval, Blocked; Goose, Gemini CLI
and Claude Code alike), deny always wins, and an organisation layer only narrows. No consumer
product says "MCP server": Claude says connector, ChatGPT app, Goose extension. Each offers a
directory with one-click connect and an OAuth sign-in.

## Decisions

**An app is connected to Ghost, once, and every department reaches it.** Apps live in
`~/.config/ghost/apps.json`, keyed by a short name: a URL for one Ghost signs in to or sends a
key to, or a command for one that runs on this computer. Secrets are still `${NAME}` from
Ghost's environment, and OAuth tokens are kept in a 0600 file per app under
`~/.local/share/ghost/apps/`. The file also reads the `mcpServers` shape other clients share,
so a config pasted from Claude or Cursor works. Departments no longer start apps of their own.

**Every tool runs.** A tool the app marks read-only is a read and is called directly. Every
other tool is a change: it goes through the gate, which resolves its level, records it in the
audit, and calls it. There are no write declarations: no pinned arguments, declared reads,
stale check or read-back. An app's own word for a tool is untrusted, so a tool that does not
say it is read-only counts as a change, and a change is audited even when the app called it
harmless.

**Permissions block; nothing else is needed.** The operator's permissions are rules in the
`[permissions]` table of `policy.toml`. Each rule is an app, an app's tool, or a tool with
argument patterns: `gmail`, `github.delete_*`, `github.issue_write(state=closed)`,
`github.*(repo!=ghost)`. Each rule sits in one list:

| List | A change it matches | A read it matches |
|---|---|---|
| `block` | not offered; refused if called | not offered; refused if called |
| `ask` | asks first (`confirm`) | runs |
| `quiet` | runs, mentioned only if asked or if it failed | runs |
| `allow` | runs and is announced (an exception to `mode`) | runs |

The order is `block`, then `ask`, then `quiet`, then `allow`, then `mode`, so the most careful
rule that matches wins. `mode` is the default for a change no rule matches: `open` (announce,
the default), `ask` (every change asks), or `read-only` (every change is blocked). That is the
lockdown. A department's `department.json` may add `block`, `ask` and `quiet` rules for its own
sessions. It may not add `allow`, so a department only ever narrows what Ghost does. Ghost's own
operations (`conversations.*`, `skills.*`, `departments.*`, `outputs.*`) resolve by the same
rules, also open by default. The older per-source tables (`[conversations] delete = "confirm"`)
still read, as rules.

**The gate stays the one path for a change**, at every level: the level from the rules, a
proposal, the operator's yes at `ask`, the call, the audit row. At `ask` the operator's plain
request is still the yes (ADR 0017). The words that ask for a change now come from the tool's
own name and its short arguments, through a fixed list of verbs (`close`, `delete`, `send`,
`merge`, …), never from the app's description. A tool whose name has none of them asks.

**The product says app.** The apps, the voice, the prompts, the tool descriptions and the
operator's docs say *app*, *connect*, *sign in* and *permission*. They do not say server, MCP
or declaration. The code keeps `mcp` only where it names the protocol library.

**Apps come from a catalog.** The desktop's Apps screen lists apps from the official MCP
registry. The registry asks host applications not to query it live, so `ghostd` keeps its
own copy, refreshed at most hourly when the screen asks. Each entry offers the ways it can
connect:

- *Sign in*: a remote app that uses OAuth. Ghost registers itself with the app's
  authorization server, opens the browser, and receives the code on a loopback address that
  listens only while a sign-in is open.
- *API key*: a remote app with a secret header. The key goes into Ghost's environment, and the
  file names it by reference.
- *Runs on this computer*: an npm, PyPI or container package, started through `npx`, `uvx` or
  `docker`, with its settings as environment variables.

An app not in the catalog is added by URL, or by pasting another client's config, behind an
"Add your own" door. Catalog text is shown to the operator and never reaches the model.

## What does not change

- Everything an app sends is data: descriptions (first sentence only), results, notices. None
  of it changes a permission, a prompt or a level, and the model is told not to repeat it.
- The model never writes `apps.json`, `policy.toml`, a token, or a department's rules except by
  appending the operator's words through the gate (ADR 0024).
- Secrets go into the control API and never come out. A sign-in's tokens never leave `ghostd`.
- Watches and webhooks read through an app's read-only tools, and only through those (ADR 0027,
  ADR 0028). Their declarations say `app` where they said `server`.

## Consequences

- `ghost_core.apps` replaces the server half of `ghost_core.mcp`, `ghost_core.permissions`
  replaces the levels half of `ghost_core.policy`, and `ghost_core.catalog` and
  `ghost_core.signin` are new. `ghost_core.scope` goes, and with it the allowlists, ownership
  and `GHOST_DEPARTMENT_ROUTING`. A department limited to some repositories says so with a
  `block` rule (`github.*(repo!=site)`).
- The control API: `/apps` (list, connect, disconnect, sign in), `/catalog` and
  `/permissions`; `/mcp` goes. The desktop's Servers screen becomes Apps, and the Policy
  screen becomes Permissions.
- Performance (ADR 0018): every tool of every app is now in the prompt. The GitHub server
  with every toolset offers about a hundred, which costs time to the first token on every
  turn. Measure with `make perf` after connecting an app. Where it costs too much, narrow the
  app by its own arguments (GitHub's `--toolsets`), or `block` the tools that aren't wanted.
- What is lost: the stale check and read-back on GitHub's writes, and the hidden marker on a
  created comment. The audit row says a change was not checked. Asked to bring the check back
  for one app, write it as a check on that app, not as a declaration.
- No migration (as in ADR 0024). Ghost's `mcp.json` and the departments' `mcp.json` and
  `servers` are not read any more. `ghostd check` names them, and the operator connects the
  apps again from the catalog or copies the entries into `apps.json`.

## Considered options

- **Keep declarations as optional extras.** Rejected by the operator: two ways to write the
  same thing, and the declared way is the one nobody writes.
- **Per-department apps, as before.** Rejected: an app is connected to Ghost the way an
  account is. A department narrows it, and does not own it.
- **Ask first for tools the app marks destructive.** Rejected: the hint is the app's word and
  is untrusted, and the spec makes it default to true, so it would ask for nearly everything.
  An `ask` rule is the operator's word.
- **Query the registry live from the desktop.** Rejected: the registry asks hosts not to, and
  it is in preview without uptime guarantees.
