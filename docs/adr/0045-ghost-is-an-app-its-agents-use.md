# 0045: Ghost is an app its agents use

- **Status:** accepted; amended by ADR 0048 (the same server is served to outside clients)
- **Date:** 2026-11-11
- **Amends:** ADR 0034 and ADR 0035 (the agents, the rules, the outputs, the skills and
  Ghost's fixed intents are no longer the only in-tree tool sources), ADR 0013 (the model
  now has a route to Ghost's own state, through tools, never through the control API),
  ADR 0018 (nine more tools on every turn; measured below)

An agent living in Ghost could not say how Ghost works, could not see what Ghost was
doing, and had no way to reach the operator except by speaking while they were there.
Asked "can you watch my CI?", it answered from what its session happened to offer. The
functions existed, behind the control API, which the model has no route to by design.

## Decision

Ghost's own functions are one MCP server (`ghost_core::ghost_app`, on rmcp), and the
daemon serves it to its own agents in process: a `tokio::io::duplex` pair, the server on
one end, the ordinary app client on the other, joined to the apps through the seam an
already open session uses (`App::attach`, `Apps::attach`). It is one more app, named
`ghost`. Its tools are offered as `ghost_<tool>` by the same naming, a tool marked
read-only is a read and every other tool is a change through the gate, the audit records
each change, and the operator's `block`, `ask`, `quiet` and `allow` rules apply to
`ghost.*` as to any app.

The first tools:

| Tool | Kind | What it does |
|---|---|---|
| `docs_search(query)` | read | The best sections of Ghost's own documentation, with a short excerpt each |
| `docs_read(name, section?)` | read | One section, or a document's headings |
| `status()` | read | Version, phase, the session and its agent, the apps and their state, how much is waiting, the model connection's provider and model, Ghost's own problems |
| `audit_recent(limit?, since?)` | read | Recent changes: what, which agent, when, how it ended |
| `apps_list()` | read | The connected apps, their state and how many reads and changes each offers |
| `notify(title, detail?, urgency?)` | change | An item from the calling agent on the waiting list; the routes deliver it |
| `resolve(key)` | change | Clears such an item; an agent clears only its own |
| `waiting_ack(keys)` | change | The control API's `POST /waiting/ack` |
| `waiting_snooze(keys, minutes)` | change | The control API's `POST /waiting/snooze` |

The documentation is a curated set built into the binary (the README, the glossary,
`architecture.md`, `apps.md`, `skills.md`, `folder.md`, `performance.md`,
`phone-protocol.md`), split by heading and searched by counting words: no model, no
network. The ADRs, the design system, the original documents and the direction draft are
left out; they say how Ghost was decided, not how it works.

What differs from an app the operator connected, and why:

- **Its text is Ghost's own.** Descriptions reach the model whole, and results are not
  trimmed to 1,200 characters and 30 items; the tools cap themselves (a section at 8,000
  characters, said when cut; the audit at 50 rows). Every other app is trimmed as before.
- **The caller is not an argument.** `notify` and `resolve` need the calling agent. The
  session's dispatcher sets `_agent` to the agent the session is with, over anything the
  model wrote, the way it sets `_request` for `rules.add`; the argument is not in the
  schema the model sees, is dropped for every other app, and is left out of what a
  permission rule can match, so it cannot become a permission per agent (ADR 0035).
- **Its changes are quiet by default.** An item on the waiting list is its own
  announcement. Where no rule names a `ghost.*` change and the mode is `open`, it runs at
  `quiet`: done, recorded, not a sentence of its own. `allow = ["ghost.notify"]` has
  Ghost say it, `ask` asks, `block` takes the tool away, and the `ask` and `read-only`
  modes hold for it as for everything else.
- **It is not in `apps.json`.** `ghost` is a reserved name, so an entry cannot shadow
  it (`ghostd check` names one that tries), and Ghost's own app is listed first, so an
  app called `ghost_docs` cannot take `ghost_docs_search`. It starts before any session
  can open, needs no vault, network or sign-in, is never an app that "isn't connected" on
  the waiting list, and is started again on the same app if its task stops.

What an agent tells the operator is an item under the source `agent:<slug>`, keyed by
the agent and the title, so the same thing said twice is one item. The digest, desk voice, the desktop
notification and the phone deliver it with the quiet hours, batching, acknowledgment and
escalation they already have; `urgency` (`digest`, `notify`, `call`) is the ceiling the
routes read for it.

The system prompt gains the app's paragraph: look Ghost up with `ghost_docs_search`
before answering how it works or saying it cannot do something, and use `ghost_notify`
to tell the operator something later. It is in the prompt when the tools are.

## Consequences

- Two rules of the operating manual are amended, deliberately. The in-tree tool sources
  are no longer a closed list: Ghost's own functions come as tools of its own app. And
  the model has a route to Ghost's state, which is this server's tools and nothing else:
  the control API is still not reachable from a model, and still carries no passive
  audio, no passive transcript and no password out.
- The gate rules stand. No change reaches Ghost's state from a model except through the
  gate; no tool here sets a permission, a rule, a watch or a prompt; nothing here reads a
  password or a transcript.
- An agent's item is the agent's words on the operator's list. Its title is shown and
  spoken as any item's is: data, bounded to 200 characters.
- Cost on a turn (ADR 0018), measured by
  `ghosts_own_app::what_ghosts_own_tools_add_to_every_turn`, a session with no app the
  operator connected: the advertised tool list goes from **8 tools and 5,731 bytes** of
  JSON schema to **17 tools and 8,929 bytes** (+9 tools, +3,198 bytes, about 800 tokens),
  and the system prompt gains 492 bytes. All of it is in the cached prefix. A tool the
  operator blocks leaves the list. The test fails if the nine grow past 3,500 bytes.
- A call to Ghost's own app stays in the process: it is not acknowledged aloud ("On
  it.") the way a call to a connected app is.

## What follows

- **The same server for outside clients.** The handler is `Clone` over shared state and
  keeps nothing per connection, so it can be served over streamable HTTP with a token.
  The listener must name the caller from what it authenticated (`Caller::Named`); it
  never believes an argument. Nothing is listening in this increment.
- **More tools on this server**, each a read or a change through the gate: schedules,
  managing the apps, an agent's reads of its own memory, and an `ask` that waits for the
  operator's answer.

## Alternatives considered

- **More in-tree `ToolSource`s, as the waiting list has.** That is how `waiting_list`
  came in, and each one is code in the pipeline's reach that no outside client could ever
  use. One server gives the agents and, later, the operator's other tools the same
  functions under the same gate.
- **Let the model call the control API.** It has routes a model must never reach (the
  permissions, the passwords, Confirm), and none of them goes through the gate.
- **The caller as an ordinary argument.** A model would name another agent as easily as
  itself; an agent could then clear what it did not write.
