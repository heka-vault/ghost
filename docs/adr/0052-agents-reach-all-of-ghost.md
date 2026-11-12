# 0052: Agents reach all of Ghost, through Ghost's own app

- **Status:** accepted (decided 2026-11-11 by the owner; built 2026-11-12 on the `feat/own-tools-2` branch: 18 tools on Ghost's own app, the recall index of ADR 0041, the deferral of Ghost's own tools, the written style for typed turns; what is not built is listed below)
- **Date:** 2026-11-12
- **Builds on:** ADR 0045 (Ghost is an app its agents use), ADR 0046 (deferred tools), ADR 0047 (the run with nobody there), ADR 0048 (outside tools), ADR 0051 (questions)
- **Amends:** ADR 0028 (an agent may write `watches.json`), ADR 0033 (outputs: more kinds, 1 MB, read and deleted by a tool), ADR 0034 and ADR 0035 (an agent may connect an app and write `apps.json`, with the operator's yes; it may write a skill), ADR 0045 (a tool may set a watch; four kinds of change ask by default; most of Ghost's own tools are found by search), ADR 0014 (Ghost reads any agent's transcript and memory), ADR 0018 (the cost, below)

An agent living in Ghost could tell the operator something, ask them something, set up a
schedule and look Ghost up, and not much else. Asked "watch the deploys and tell me when one
fails", "connect Linear", "what did we say about the invoice in August", "ask Ledger whether
October is paid", "nothing until two", it had no tool, though the operator could do each of
them from the desktop. The earlier design kept most of Ghost's state away from the model on
purpose. The owner is taking Ghost to commercial quality and reversed that: everything Ghost
can do, its agents can do, through Ghost's own tools. The leading open-source agent offers
its agents about seventy tools over itself (OpenClaw's `tool-catalog.ts`, its gateway and
delegate tools; Hermes Agent's session search, memory and delegate tools; both MIT); this
takes the functions, and keeps Ghost's gate.

## Decision

**Ghost's own app gains a tool for each thing the operator could do and an agent could
not**, each a read or a change through the gate, in the audit, under `ghost.*` permissions,
the caller from the dispatcher and never from an argument:

| Area | Tools | Default with no rule |
|---|---|---|
| Recall (ADR 0041) | `recall`, `transcript_read`, `memory_read` | reads |
| Apps | `apps_catalog_search`, `app_tools` (reads); `app_connect`, `app_disconnect` | **ask** |
| Watches | `watches_list` (read); `watch_manage` | **ask** to make or change; quiet to pause, resume, remove |
| Routes and quiet | `routes_read` (read); `quiet_until`, `quiet_off` | quiet |
| Permissions and settings | `permissions_read`, `settings_read` | reads only |
| Outputs | `output_read` (read); `output_delete` | quiet |
| Agents | `ask_agent` | quiet |
| Skills | `skill_write` | **ask** |

The full list, with which outside tools see each, is [tools.md](../tools.md).

**Four kinds of change ask by default** (`ghost_app::asks_by_default`): connecting or
disconnecting an app, writing a skill, and making or changing a watch. Each runs or contacts
code the operator did not write, or adds instructions every agent follows. Every other
change of Ghost's own app stays quiet by ADR 0045. The operator's `block`, `ask`, `quiet`
and `allow` rules, and the modes, hold over both.

**What a model cannot do, the operator's part, stays the operator's.** An agent connects an
app by the desktop's own code (`catalog::connect`, `apps::address_spec` and `free_name`,
`apps::edit_apps`, `Apps::add`), but a sign-in happens in the operator's browser and a key
is typed by the operator: an app that needs either is left with an item on the waiting list
("Linear needs you to sign in") and the call says so. A key an agent tries to pass is
refused. A watch an agent makes is a polled one, recorded with `made_by`, with no Jev or
model step (those are the operator's to declare); it is written by one writer that keeps the
operator's other entries in order and refuses the whole file when it would gain a problem
`ghostd check` reports, or would read an app that is not connected or a tool that is not one
of its reads. An agent changes only the watches it made; Ghost (agent zero) any agent's; the
operator's own are theirs. Which routes are on, and their order, stay the operator's file.

**One agent asks another** (`ask_agent`) through the run with nobody there that an outside
tool's `agent_ask` uses: the asked agent's own prompt, memory and tools, the same gate (a
change at `ask` is set aside, never made: nothing an agent writes is the operator's yes), 30
model calls and ten minutes, the turn kept in its transcript as `[asked by <agent>, another
agent]` under `run-agentask-<id>`. An agent asked this way cannot ask a third (depth 1), and
a session has three running at the most. It is named `ask_agent`, not `agent_ask`, because
the gate keeps one operation per name under `ghost.` and `agent_ask` is the outside tool's.

**Ghost's own tools past a core are found by search.** Every tool is schema on every turn
(ADR 0018), and 35 of them would be 17,140 bytes. The deferral ADR 0046 made for connected
apps now covers Ghost's own app: the core (`notify`, `resolve`, `ask`, `answer`,
`waiting_ack`, `status`, `docs_search`, `recall`, `schedule_manage`, `tools_search`) is
offered by name on every turn, and the rest are found with `ghost_tools_search` and offered
by name from the next model call, or once called by name (the `Recent` list, shared with
the apps). Why each is in the core is in `ghost_app::CORE`. A hidden tool is still
registered with the gate and callable by name; hiding is for the prompt's size and never a
permission. Ghost's paragraph of the prompt says the rest are found by search.

**A reply that will be read gets the written style.** The voice prompt ("one short
sentence", no lists, no markdown) is the same for every turn, so the cached prefix holds. A
turn typed in the desktop app (`POST /session/say`, and a session the desktop opens with
typed words, `SessionParams::first_typed`) carries a short note before the operator's words
(`ghost_voice::prompt::TYPED_NOTE`): the reply is read on screen as well as heard, may be as
long as the question needs, and may use short lists. A spoken turn has no note. A turn
another agent asks for is a run, whose prompt is already the written one, and its first
message says the answer goes back to the agent that asked.

**Outside tools stay narrower** (`ghost_app::NOT_FOR_OUTSIDE`). Offered: `recall` and
`transcript_read` over the turns of the runs that client started (its `agent_ask` turns,
`run-ask-<client>-<id>`, and its schedules' runs), `watches_list` and `routes_read`. Never
offered: `memory_read`, `app_connect`, `app_disconnect`, `app_tools`, `apps_catalog_search`,
`watch_manage`, `permissions_read`, `settings_read`, `skill_write`, `quiet_until`,
`quiet_off`, `output_read`, `output_delete`, `ask_agent` (it has `agent_ask`), and
`tools_search` as before. The brief left four open; the catalog and an app's tool list are
other publishers' text with no use to a tool that cannot call an app's tool, and an output
is the operator's work, so they stay in.

**Limits that were too small for working agents are raised**: a skill's body and files are
read up to the result budget (24,000 characters, from 6,000); an output is up to 1 MB (from
256 KB) and may be any of 21 text kinds (pages and their styles and scripts, Markdown,
reStructuredText, TeX, CSV and TSV, JSON, XML, YAML, TOML, a calendar file, SVG, SQL,
Python and shell), never executable.

## What still holds

- **A model never changes a permission.** There is no tool for it; `permissions_read` is a
  read, and `no_tool_changes_a_permission` holds the line. Nothing writes `policy.toml`.
- **A model never reads a password.** `settings_read` drops every password-shaped key and
  every address, and redacts what is left; `routes_read` names whether ntfy is set up, never
  its topic. A recognisable password seeded into the settings is asserted absent.
- **`hooks.json` is never an agent's.** Webhooks keep their signatures and parsers in the
  operator's file; `watch_manage` never reads or writes it.
- **The rules file is appended only with the operator's words** (`rules.add`), as before.
- **Nothing returns a passive transcript or audio.** Only session and run turns are stored.
- **Nothing an agent or a tool writes is the operator's yes**, in a run or an asked turn.

## Consequences

- Cost on a turn (ADR 0018), measured by `what_ghosts_own_app_costs_every_turn` with the
  daemon's full server (schedules, questions, every wider part): Ghost's own app advertised
  **16 tools and 8,397 bytes** of schema before this, and **10 tools and 7,022 bytes** after
  (`schedule_manage` 2,300, `ask` 1,335, `recall` 856, `notify` 551, `tools_search` 457,
  `answer` 421, `docs_search` 306, `waiting_ack` 293, `status` 257, `resolve` 246). The test
  fails past 7,500 bytes or past ten tools. (2026-11-12: `docs_search`'s description says
  a short section comes back whole, 335 bytes, so 7,051 in all.) Ghost's paragraph of the prompt grows from 692
  to 812 bytes. A tool found by search costs one model call and one loss of the cached
  prompt when it joins the list.
- An agent that needs `docs_read` after `docs_search`, or `transcript_read` after `recall`,
  searches for it first or calls it by name; either keeps it offered for a while.
- The waiting list gets an agent's item for each app it could not finish connecting.
- The database gains the recall index (ADR 0041).

## What is not built

- `app_connect` does not offer a way for an agent to start the operator's sign-in in their
  browser; the operator presses Sign in on the Apps screen.
- (Built since, on `feat/polish-1`: the retention changes proposed in ADR 0041, and the
  desktop's "made by" and pause controls for a watch.)
