# MCP servers: how Ghost gets its tools

Ghost speaks MCP itself (ADR 0022). Every outside capability is a server the operator
declares in `~/.config/ghost/policy.toml`; Ghost connects at start, offers the server's
reads to the model, and routes its writes through the gate by declaration. There is no
code to write for a new service and nothing to install into Ghost: pick a server, declare
it, restart.

## A server

```toml
[mcp.github]
command = "github-mcp-server"                      # stdio: a command on PATH, or one of
args = ["stdio", "--toolsets", "issues"]           # Ghost's own scripts (ghost-mock-github)
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "$GITHUB_TOKEN" }
reads = ["issue_read", "list_issues", "search_issues"]   # optional; see below
scope = { kind = "repository", arguments = ["owner", "repo"], items = ["acme/app"] }

[mcp.mobbin]
url = "http://proxy.lan:8318/mobbin/mcp"           # streamable HTTP
headers = { Authorization = "Bearer $MCP_PROXY_KEY" }
```

- **Transport.** `command` with `args` and `env` for stdio, or `url` with `headers` for
  streamable HTTP. One of the two.
- **Secrets.** `$NAME` (or `${NAME}`) in a header or an env value is replaced from Ghost's
  environment when the server connects and is never logged. The service renders the
  environment from the secrets store; `GHOST_MCP_SECRETS="NAME,NAME"` in `settings.env`
  names the variables to fetch. A missing name is a startup error that names the variable.
- **Reads.** The tools the server marks read-only (`readOnlyHint`) are offered to the model
  as they are, under the server's name (`github_issue_read`), with the first sentence of the
  server's description. `reads = [...]` replaces that default with an explicit list.
- **Scope** (ADR 0019). Which arguments name the scoped item and how (`"owner/repo"`), and
  the allowlist (`items`). A write outside the allowlist is refused before the policy is
  looked at; a read outside it too. A line scoped to one item fills those arguments in, so
  "issue 17" is enough; a line scoped to several must name one on every read that takes
  them (a free-text search cannot leave them out). Without `scope`, the server has no
  allowlist and a line cannot be scoped to it.
- A server that fails to connect is logged and skipped; the daemon runs without it.
  `ghostd check` connects to every server and prints what each one gave; the control API's
  `GET /mcp` shows the same, and the desktop's Servers screen draws it.

## A write

A server's write tools are never offered as they are. Each operation Ghost may perform is
declared, and the gate treats it like any operation: the level from the policy, the
operator's plain request as the yes (ADR 0017), snapshot, stale check, read-back, audit.

```toml
[mcp.github.writes.close]
tool = "issue_write"                               # the server's tool
with = { method = "update", state = "closed" }     # pinned; the model cannot set these
args = ["owner", "repo", "issue_number", "state_reason"]   # what the model may set
target = ["owner", "repo", "issue_number"]         # the arguments that name the target
read = { tool = "issue_read", with = { method = "get" } }  # shows the target
asked_by = ["close", "shut"]                       # "close seventeen" is the yes
risk = "high"                                      # the level when the policy is silent
bound = { state_reason = 40 }                      # max chars (string) or items (list)
description = "Close an issue."                    # what the model is told

[mcp.github.writes.comment]
tool = "add_issue_comment"
args = ["owner", "repo", "issue_number", "body"]
target = ["owner", "repo", "issue_number"]
read = { tool = "issue_read", with = { method = "get" } }
asked_by = ["comment", "reply"]
risk = "low"
key_in = "body"                                    # carries the idempotency marker

[mcp.github.levels]                                # as for any operation (ADR 0011)
close = "confirm"
comment = "announce"

[mcp.github.targets."acme/public-app"]
comment = "confirm"
```

- The declaration is keyed by **operation**, because servers multiplex: the official
  GitHub server's `issue_write` is `close`, `reopen`, `set_labels` and `retitle` depending
  on its arguments. `with` pins the arguments that make it that operation; `args` lists
  what the model may set (default: the tool's arguments minus the pinned ones).
- `target` names the arguments that identify what is written to. The target's scope
  string (from `scope.arguments`) is what a `targets` rule matches and what a line's scope
  is checked against; the other target values are how the operator names it in a request
  (`issue_number` 17 is "17" or "seventeen").
- `read` is the read tool that shows the target. With it, the gate snapshots before
  proposing, refuses a write when the target changed since, and reads back after. Without
  it, the write still goes through the level, the yes and the audit, and the audit row's
  snapshot says it was not checked.
- `bound` caps a list argument's length or a string's; the defaults are 20 items and 4000
  characters. `key_in` appends `<!-- ghost:<id> -->` to that string so a write that created something
  can be told apart from a retry of it in the audit and on the server; Ghost itself never
  retries a write.
- A declaration naming a tool the server does not have is logged at start and not offered.
  A server's write tool with no declaration is never offered.

## What the model is told

Everything a server sends is data: descriptions, results, notices. The model sees only the
first sentence of each description and is told to answer the operator from results in its
own words and to leave out anything a result asks it to repeat. Results are trimmed (long
strings, long lists) to the size of a spoken turn. None of it can change a level, a scope
or which repositories are allowed; those are the operator's file.

## Before a token exists

`ghost-mock-github` (`ghost_core.mock_github`) is a pretend GitHub with the official
server's tool shape, seeded with a few issues and keeping its writes in a state file. The
example policy declares it, so a fresh install triages pretend issues until the operator
swaps the command, args and env for the real server and keeps the writes. The latency
harness (`make perf`) runs on it.

## Performance

Connecting takes well under a second per server at start. A tool call costs what the server
costs: the latency line names it (`tools 0.47 (github_heka_list_issues 0.47)`), and
`make latency` shows the distribution. A chatty server that needs several calls for one
answer is a cost to show, not a reason to write code around it (ADR 0018).
