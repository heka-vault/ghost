# MCP servers: how Ghost gets its tools

Ghost speaks MCP itself (ADR 0022). Every outside capability is a server a department
declares in its folder (ADR 0024): `mcp.json` says how to reach it, `department.json`
says what the gate may do with it. Ghost's own servers are declared in `~/.config/ghost/`
itself and connect at start; a department's connect on the first transfer into it. There
is no code to write for a new service and nothing to install into Ghost: pick a server,
declare it, and restart Ghost (a department's folder is re-read when the desktop's
Departments screen refreshes or the policy file reloads; its servers start at the next
transfer into it).

## A server

`mcp.json` is the shape other clients share, so a server copies to and from Cursor or
Claude unchanged:

```json
{
  "mcpServers": {
    "github": {
      "command": "github-mcp-server",
      "args": ["stdio", "--toolsets", "issues"],
      "env": { "GITHUB_PERSONAL_ACCESS_TOKEN": "${GITHUB_TOKEN}" }
    },
    "mobbin": {
      "type": "http",
      "url": "http://proxy.lan:8318/mobbin/mcp",
      "headers": { "Authorization": "Bearer ${MCP_PROXY_KEY}" }
    }
  },
  "use": ["github"]
}
```

- **Transport.** `command` with `args` and `env` for stdio, or `type: "http"` with `url`
  and `headers` for streamable HTTP.
- **Secrets.** `${NAME}` (or `${env:NAME}`, or `$NAME`) in a header or an env value is
  replaced from Ghost's environment when the server connects and is never logged. A value
  that looks like a literal token is a problem the folder reports; put it in the
  environment. The service renders the environment from the secrets store;
  `GHOST_MCP_SECRETS="NAME,NAME"` in `settings.env` names the variables to fetch, and the
  desktop's Settings can set them. A missing name is a startup error that names the
  variable.
- **`use`** (a department's `mcp.json` only) reaches one of Ghost's servers under its own
  name instead of starting another process; the department's own `department.json` entry
  for it says what that department may read and write and at what level, and its
  allowlist can only be Ghost's or a subset of it.
- A server that fails to connect is logged, reported as a problem of its department, and
  skipped; the daemon runs without it. `ghostd check` connects to Ghost's servers and
  prints what each one gave and every department's problems; the control API's `GET /mcp`
  and `GET /departments` show the same, and the desktop's Servers and Departments screens
  draw them.

## What the gate may do with it

`department.json` is keyed by server name. Each entry carries what ADR 0022 declared, with
the same meaning:

```json
{
  "title": "Triage",
  "aliases": ["issues"],
  "voice": "bm_george",
  "servers": {
    "github": {
      "reads": ["issue_read", "list_issues", "search_issues"],
      "scope": { "kind": "repository", "arguments": ["owner", "repo"], "items": ["acme/app"] },
      "writes": {
        "close": {
          "tool": "issue_write",
          "with": { "method": "update", "state": "closed" },
          "args": ["owner", "repo", "issue_number", "state_reason"],
          "target": ["owner", "repo", "issue_number"],
          "read": { "tool": "issue_read", "with": { "method": "get" } },
          "asked_by": ["close", "shut"],
          "risk": "high",
          "bound": { "state_reason": 40 },
          "description": "Close an issue."
        },
        "comment": {
          "tool": "add_issue_comment",
          "args": ["owner", "repo", "issue_number", "body"],
          "target": ["owner", "repo", "issue_number"],
          "read": { "tool": "issue_read", "with": { "method": "get" } },
          "asked_by": ["comment", "reply"],
          "risk": "low",
          "key_in": "body"
        }
      },
      "levels": { "close": "confirm", "comment": "announce" },
      "targets": { "acme/public-app": { "comment": "confirm" } }
    }
  }
}
```

- **Reads.** The tools the server marks read-only (`readOnlyHint`) are offered to the model
  as they are, under the server's name (`github_issue_read`), with the first sentence of the
  server's description. `reads` replaces that default with an explicit list.
- **Scope.** Which arguments name the scoped item and how (`"owner/repo"`), and the
  allowlist (`items`). A write outside the allowlist is refused before the policy is looked
  at; a read outside it too. The allowlist also says what the department **owns**: a
  request about one of its items from another department gets the owner's name and an offer
  to transfer (`GHOST_DEPARTMENT_ROUTING=1` transfers and hands the request over).
- **Writes.** A server's write tools are never offered as they are. Each operation is
  declared, keyed by **operation** because servers multiplex (the official GitHub server's
  `issue_write` is `close`, `reopen`, `set_labels` and `retitle` depending on its
  arguments). `with` pins the arguments that make it that operation; `args` lists what the
  model may set (default: the tool's arguments minus the pinned ones); `target` names the
  arguments that identify what is written to (the scope string is what a `targets` rule
  matches; the rest is how the operator names it, `issue_number` 17 is "seventeen");
  `read` is the read tool that shows the target, so the gate snapshots before proposing,
  refuses a write when the target changed since, and reads back after (without it the
  write still goes through the level, the yes and the audit, and the audit row says it was
  not checked); `asked_by` are the words that make the operator's plain request the yes
  (ADR 0017); `risk` is the level when the policy is silent; `bound` caps a list's length
  or a string's (defaults 20 items, 4000 characters); `key_in` appends `<!-- ghost:<id> -->`
  to that string so a write that created something can be told apart from a retry in the
  audit and on the server (Ghost itself never retries a write).
- **Levels** and **targets** are as for any operation (ADR 0011): `deny`, `confirm`,
  `announce`, `quiet`, per operation and per target item. The policy file's `[defaults]`
  still applies underneath; a department's levels are its own, over those defaults.
- A declaration naming a tool the server does not have is logged at start and not offered.
  A server's write tool with no declaration is never offered.

## What the model is told

Everything a server sends is data: descriptions, results, notices. The model sees only the
first sentence of each description and is told to answer the operator from results in its
own words and to leave out anything a result asks it to repeat. Results are trimmed (long
strings, long lists) to the size of a spoken turn. None of it can change a level, a scope
or which repositories are allowed; those are the operator's files, and the model has no
tool that writes them.

## Before a token exists

`ghost-mock-github` (`ghost_core.mock_github`) is a pretend GitHub with the official
server's tool shape, seeded with a few issues and keeping its writes in a state file.
`infra/example/` declares it as Ghost's `github`, and `make env` seeds a fresh install
with it, so Ghost triages pretend issues until the operator swaps the command, args and
env for the real server and keeps the declarations. The latency harness (`make perf`)
runs on it.

## Performance

Connecting takes well under a second per server at start. A department's own servers
start on the first transfer into it; that start is logged with its seconds and is a cost
on that turn, and they stay up for ten minutes after the last session leaves so the next
transfer pays nothing. A tool call costs what the server costs: the latency line names it
(`tools 0.47 (github_list_issues 0.47)`), and `make latency` shows the distribution. A
chatty server that needs several calls for one answer is a cost to show, not a reason to
write code around it (ADR 0018).
