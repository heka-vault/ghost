# 0048: Outside tools connect to Ghost's own app, each with a name and a token

- **Status:** accepted (decided 2026-11-11; built on the `feat/ghost-app-outside` branch: the listener, the clients and their tokens, the four tools, the bridge, Settings › Outside tools; `agent_ask` runs through ADR 0047's run with nobody there, and the schedules are offered to outside clients as their own)
- **Date:** 2026-11-11
- **Amends:** ADR 0045 (the same server, now also served to outside clients), ADR 0032
  (an outside tool's sentence may be what desk voice says), ADR 0013 (a second way in for
  the operator's own tools, which is not the control API)

The operator works in other tools on the same computer: Claude Code, Cursor, Codex. They
finish a build or need an answer while the operator has looked away, and they had no way
to reach them. Ghost already has that way (the waiting list, the routes, its voice), and
since ADR 0045 it has it as one server its own agents use. This serves that server to the
operator's other tools, so "tell me when the build is done" works from any of them.

## Decision

`ghostd` serves Ghost's own app (`ghost_core::ghost_app`) over Streamable HTTP on a
listener of its own (`ghostd::outside`): one path, `/mcp`, through rmcp's
`StreamableHttpService`, which answers the session-based clients and the stateless
2026-07-28 ones alike. The handler keeps nothing per connection; who is calling comes with
each request.

**Its own addresses, this computer by default.** `GHOST_MCP_LISTEN` is `host:port`, comma
separated; unset it is `127.0.0.1:47824`, and empty turns the listener off. It shares no
route with the control API (ADR 0013), the phone listener (ADR 0016) or the webhook
listener (ADR 0027), and anything but `/mcp` is a 404. It starts after Ghost's own app is
attached. An address that will not bind is an item on the waiting list, not a crash. An
address that is not loopback is allowed only because the operator wrote it, and is logged
as such: the token then crosses that network unencrypted, so it belongs on the operator's
own WireGuard network (ADR 0020) and nowhere wider.

**No browser.** A request that carries an `Origin` header is refused before anything else:
a browser sends one and a tool does not, so a page that reaches the port by DNS rebinding
gets nothing. rmcp's `Host` allow-list is kept behind it (this computer's names, and any
other host the operator configured).

**A client is a name and a token.** The operator connects a tool by name (`ghostd mcp add
<name>`, or Settings › Outside tools, which is `POST /clients` on the control API) and is
shown its token once, with the line to paste into the tool. The token is
`ghost.<name>.<32 random bytes>`; the store keeps its SHA-256 with the name and when it was
made and last used (`outside_clients`), and nothing returns it again. It is a password in
Ghost's sense (ADR 0036): never logged, never in the audit, compared in constant time.
Every request must carry one. Five wrong tokens for a name refuse that name for five
minutes (the limits of ZeroClaw's pairing guard); a request that names nobody is counted
against its address. Because a token names its client, one tool with a stale token cannot
lock the others out, and a token made after a lockout began lifts it. Removing a client
(`ghostd mcp remove`, `DELETE /clients/{name}`) is a delete in the store the listener reads
on every request, so it is refused at once.

**The caller is what was authenticated.** The listener puts the client's name in the
request's own extensions and the server names the caller from that: `client:<name>`, the
`Caller::Named` ADR 0045 left for this. An argument is never believed, and a `_`-led one a
client wrote is dropped.

**Every call is accountable.** In process a change reaches the server through the
session's dispatcher and the gate. A call from outside has no session, so the server does
the equivalent itself (`ghost_app::outside`):

- A tool marked read-only is a read: refused when a `block` rule names it, logged at debug
  level, otherwise called.
- Every other tool is a change through the gate's own `submit`, as `ghost.<tool>`, with
  the operations Ghost's own app already registered there. `block` refuses it. `ask`
  leaves it pending on the waiting list for the operator's yes (a click in the desktop, or
  a spoken yes once Ghost reads it out) and the client is answered "waiting for the
  operator": nothing a client sends is ever that yes. Anything else runs. Either way the
  audit has the proposal and its outcome under the session `client:<name>`.
- Each client may make 60 calls a minute, and of those 10 that tell the operator
  something (`notify`, `say`). Past that the call is refused with how long to wait.
- A tool an unconditional `block` names is not listed.

**What a client tells the operator is its own.** A `notify` from an outside client is an
item under the source `client:<name>`, its title led by who it is from ("From Claude Code:
the build finished"), so every route says so. A client resolves only its own items.

**Four more tools, for outside clients only.** Ghost's own agents are not offered them, so
they cost a session nothing (ADR 0018).

| Tool | Kind | What it does |
|---|---|---|
| `agents_list()` | read | The agents, their jobs, and whether the operator is in a session with one |
| `agent_ask(agent, text)` | change | A written turn for an agent and its written answer; nothing is spoken |
| `say(text)` | change | One sentence at the desk now, when the desk is free; otherwise it is a `notify` |
| `events_wait(since?, timeout_s?)` | read | A long poll, 60 s at most, for what changed |

`agent_ask` needs a way to run an agent's turn with no audio. The typed turn
(`POST /session/say`) cannot give it: with no session it opens one at the desk, and with
one it speaks the answer aloud. So the run is behind a small trait (`ghost_app::outside::AgentRunner`), and what the
daemon plugs in is the run with nobody there that came with schedules (ADR 0047:
`ghostd::outside::AskRunner` over `ghost_voice::headless`). The client's text is the
agent's instruction: the agent's own prompt, memory and tools, the same dispatcher and
gate, 30 model calls and ten minutes at the most. A change that resolves to `ask` inside
it is set aside on the waiting list and the agent says so, because nothing a client
writes is the operator's yes. The question and the answer are kept in that agent's
transcript, marked `[asked by <tool>, an outside tool]`, under a run's session id. The
turn stops when the client cancels the request or drops the connection, and a change the
agent has in the gate at that moment still gets its outcome. Without a runner the tool
answers "not available yet".

`say` goes through desk voice (ADR 0032): the passive loop takes it as it takes a raised
item, and the sentence, led by who it is from, is the greeting. It speaks only where the
operator already let Ghost speak first (`desk` in `[routes].enabled`), outside quiet hours
and do-not-disturb, with no session, hold or call under way. In every other case, and
always first, it is an item on the waiting list. A reply acknowledges it, "not now" snoozes
it, silence leaves it for the next route.

`events_wait` reads a numbered ring the daemon fills from its own event bus, the events the
control API's WebSocket sends: a waiting-list item added, changed, acknowledged, snoozed
or resolved (found by comparing the list with the last look), a change's outcome, a
session starting or ending. A log line, a turn, the voice's level, the Hearing test and an
app's sign-in address are dropped before they are parsed. A client reads its own items'
titles and what came of its own changes; of another source's item or change it learns
only that it happened. The daemon's
subscription is a tap beside the bus, so it is not counted as the desktop being open
(ADR 0031's "at the desk").

The documentation and the status are also resources: `ghost://docs/<name>` and
`ghost://status`, under the same checks as `docs_read` and `status`. They are advertised
to outside clients only: Ghost offers an app's resources to its agents as two more read
tools (ADR 0046), and its own agents already have `ghost_docs_read` and `ghost_status`.

**A bridge for tools that only start commands.** `ghostd mcp serve --client <name>` speaks
the protocol on stdin and stdout and forwards each message to the listener with that
client's token, which `ghostd mcp add <name> --command` wrote to
`~/.local/share/ghost/clients/<name>.token` (0600, in a 0700 folder; a file others can read
is refused). It goes through the listener, not a socket of its own: one way in, so the
token, the lockout, the budget and the audit are the same for every client, where a peer
on a socket would only be "this user". It keeps trying for 15 seconds a daemon that is not
up yet, says the client's hello again to one that restarted, and answers a clear error
when there is none or the token is refused.

## What is and is not reachable

Reachable, each through the checks above: the nine tools of ADR 0045, the four here, the
two for schedules (ADR 0047), the built-in documentation, and the status. `tools_search`
(ADR 0046) is not offered: it returns the connected apps' own text for tools a client
cannot call.

**Schedules.** `schedules_list` and `schedule_manage` go the same way as every other
tool: the read against the `block` rules, the change through the gate as
`ghost.schedule_manage` under the session `client:<name>`. A schedule a client sets up
is made by `client:<name>` and runs as the agent it names, Ghost when it names none. A
client updates, pauses, resumes, removes and runs only what it set up; of every other
schedule it reads the id, the agent, when it runs and whether it is on, not its name,
its instruction or what its runs said.

Not reachable, and a test asserts the tool list has none of it: passive audio or passive
transcripts, a session's transcript or an agent's memory (an agent is asked through
`agent_ask`, never read), a password's name or value, the policy file, and any way to
change a permission, an app, a rule, a watch, a prompt or a client. Clients are made and
removed by the operator only, from the command line or the control API, which a model
still has no route to.

## Consequences

- `ghost.*` permissions now speak for outside tools as well as agents:
  `block = ["ghost.say"]` silences every tool, `ask = ["ghost.notify"]` asks first.
  There is one set of permissions; nothing narrows it per client (ADR 0035).
- An outside tool's title is its own words on the operator's list and in Ghost's voice:
  data, bounded to 200 characters, never a prompt.
- ADR 0032 said the greeting says nothing about the item. `say` is the one case where it
  does, because the operator's own tool asked for exactly that sentence and the operator
  enabled desk voice. A watch's item is still only greeted.
- The token sits in the other tool's config in plain text, as that tool keeps every such
  key. It can reach nothing but the list above, and removing the client ends it.
- Two commands and one setting carry the protocol's name (`ghostd mcp`,
  `GHOST_MCP_LISTEN`) because that is the word the other tools' own commands and configs
  use. The desktop says *outside tools* and *connect* (ADR 0034).

## Alternatives considered

- **A Unix socket for local clients.** No token to keep, but most clients speak HTTP or
  start a command, and a socket's peer is a user, not a named client: no per-client audit,
  budget or removal.
- **Let outside tools use the control API.** It has routes a tool must never reach (the
  passwords, the permissions, Confirm), and none of them goes through the gate.
- **OAuth.** The operator and the tool are on one computer; a token shown once is what the
  tools' configs take, and it is revocable by name.
- **A second runner for `agent_ask`.** One headless run is being built with schedules;
  two would drift.
