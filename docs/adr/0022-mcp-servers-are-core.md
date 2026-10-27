# 0022: MCP servers are core; abilities are retired

- **Status:** accepted (implemented 2026-10-20: reads, the write declaration, the GitHub server at parity, abilities retired); the write declarations and the reads list superseded by ADR 0034 (apps connected to Ghost, every tool offered)
- **Date:** 2026-10-20
- **Supersedes:** ADR 0006 and ADR 0012 (capabilities as modular skills, then as installable abilities)

Every integration so far is an ability: a Python package with its own client, models, mock
and typed operations. The GitHub one is about 1,100 lines, most of it plumbing that every
service would repeat. The operator asked whether Markdown skills could replace abilities; the
review (2026-10-20) found that what a skill cannot give is the one thing Ghost's rules rest on,
a write path the gate can bound, check and audit, and that breadth should come from MCP.
The operator's decision: MCP support is part of Ghost's core, and abilities do not stay as a
concept. What an ability used to provide is either core (Ghost's own intents, the lines, the
pipeline) or a configured MCP server with a declaration.

## Decisions

**Ghost speaks MCP itself.** `ghost_core.mcp` connects to the servers the operator lists,
reads their tool lists, and offers the tools to the model. Servers are configured, not
installed: a `[mcp.<name>]` section in the policy file with the transport (a stdio command, or
HTTP such as the fleet's MCP proxy), the environment variables that carry its secrets, and the
declarations below. Secrets still come only from the environment. Nothing is discovered;
the operator pins each server.

**Reads are offered as they are**, one typed call each, with the same latency budget as a
native tool.

**Writes go through the gate, by declaration.** A server's write tools are listed by name
with, for each: the policy operation it counts as (so the autonomy level, `deny`, `confirm`,
`announce`, `quiet`, resolves as for any operation, per target where the policy says so), the
words an operator uses to ask for it (`asked_by`, so a plain request is the yes, ADR 0017), the
argument that names the target, and the read tool that shows that target. With the read
declared, the gate keeps snapshot, stale check and read-back; without it, the gate keeps the
level, the spoken yes and the audit, and the audit entry says which. A write tool not declared
is never offered to the model. Bounds (how much one change may touch) are declared per write
tool as well.

**Scope by argument.** A server declares which argument names the scoped item and the scope
kind, so a scoped line (ADR 0019) sees and touches only its items through MCP tools too. Ghost
stays unscoped.

**Tool descriptions are untrusted text**, like issue bodies: shown to the model, never a
source of permission.

**What abilities provided becomes core or goes away.** Ghost's fixed intents (hang up, hold,
cancel) and the lines (ADR 0014) are core already and stay so, without the ability shape. The
pipeline hook slots (interceptors, processors by slot, listeners, CLI commands) were there for
third parties; with no third-party abilities they are internal plumbing or are removed. The
`ghost.abilities` entry point, `Contributions`, `GhostContext` and `ghost_core.testing`'s
ability harness go when the GitHub ability goes. The desktop's Abilities screen becomes
Servers: each server, its tools, which are writes, and their levels.

## As built

- The declaration lives in the policy file, the operator's one file for what Ghost may
  do: `[mcp.<name>]` for the transport, `scope` and `reads`; `[mcp.<name>.writes.<op>]`
  per operation; `[mcp.<name>.levels]` and `[mcp.<name>.targets."x"]` for the levels.
- A declaration is keyed by **operation**, not by tool, and may pin arguments (`with`):
  the official GitHub server multiplexes its writes behind a `method` argument
  (`issue_write` with `method = "update"` and `state = "closed"` is `close`). Pinned
  arguments are never the model's to set; `args` narrows what it may set; `bound` caps a
  list's length or a string's; `key_in` names the string argument that carries the
  idempotency marker for a write that creates something (a comment).
- Tools are offered to the model under the server's name (`github_heka_issue_read`) so two
  servers with the same tool coexist, and with only the first sentence of the server's
  description: the rest is often instructions aimed at agents, and the model is told that
  everything a server sends is data. Results are trimmed to a spoken turn's size.
- A write with no declared read is applied without a stale check or read-back; its
  snapshot says so, and the audit row carries that.
- The first servers are the fleet's MCP proxy (Mobbin, HTTP) and the official GitHub
  server (stdio), one instance per token, each scoped to its repositories. A mock GitHub
  with the official tool shape (`ghost-mock-github`, `ghost_core.mock_github`) stands in
  before a token exists and drives the latency harness.
- The audit keeps its `ability` field as the name of the source (a server or
  `conversations`); renaming the column bought nothing.
- The ability interface is gone: `ghost_core.source.ToolSource` is what a server or a
  core module hands the pipeline and the gate; the entry point, the loader, the context,
  the testing harness, `[abilities]` in the policy file, `GHOST_ABILITIES`, the
  `ghostd abilities` commands, `/abilities` in the control API and the desktop's
  Abilities screen (now Servers) went with the GitHub ability. Old audit rows still read.

## Consequences

- `ghost_core.mcp`: client sessions per server, tool listing with refresh, the read/write
  split, the gate bridge (`MCPTarget`, `MCPSnapshot`, `MCPChange`, a `Writer` that calls the
  declared write and re-reads with the declared read), the scope filter, `asked_by` per tool.
- The control API lists servers and tools (read-only); the policy editor gains a per-tool
  level; the Abilities screen is replaced.
- Performance (ADR 0018): the per-call hop to a server process is measured by the harness; a
  chatty server that needs several calls for one answer is a cost to show, not a reason to
  write code around it.
- Order of work, as done on 2026-10-20: reads only against the fleet proxy; the write
  declaration with the gate; the GitHub MCP server beside the GitHub ability, judged by the
  audit (a comment on heka-vault/ghost#1 through the declared operation) and the latency
  line (a list read in 0.5 s, a comment in 2.4 s through the gate); then the ability and
  the ability interface retired, CLAUDE.md and docs/mcp.md rewritten.
