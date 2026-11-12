# Ghost's own tools

Every agent reaches what Ghost can do through Ghost's own app (ADR 0045, ADR 0052): its
tools reach the model as `ghost_<tool>`, a read is called directly, and every other tool is
a change through the gate, in the audit, under your `ghost.*` permissions like any app's.
This page lists them by area, one line each, with the level a change runs at when no rule
of yours names it. A read has no level: only a `block` rule stops it.

Most of them are not in an agent's list on every turn. A small core is (marked **core**);
the rest are found with `ghost_tools_search` and offered by name from then on, so the
tools Ghost has do not make every turn longer (ADR 0018). An agent that needs one and does
not see it searches first.

An outside tool you connected to Ghost (Claude Code, Cursor) sees a narrower set: see
[connect-ghost.md](connect-ghost.md). The last column says which of these it is offered.

## Reaching the operator

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `notify` **core** | change | quiet | An item for you on the waiting list; your routes bring it to you | yes |
| `resolve` **core** | change | quiet | Clears an item the agent put there | yes |
| `ask` **core** | change | quiet | Asks you something you may answer later, anywhere | yes |
| `answer` **core** | change | quiet | Records your answer, in the words you just said | no |
| `question`, `questions_list` | read | | A question asked, its state and your answer | yes |
| `question_cancel` | change | quiet | Takes back a question still open | yes |
| `waiting_ack` **core** | change | quiet | Marks items as heard | yes |
| `waiting_snooze` | change | quiet | Keeps quiet about items for a while | yes |

## Looking Ghost up

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `docs_search` **core** | read | | Searches this documentation | yes |
| `docs_read` | read | | Reads one section of it | yes |
| `status` **core** | read | | Version, phase, the session, the apps, what is waiting | yes |
| `audit_recent` | read | | What was changed lately, by which agent | yes |
| `tools_search` **core** | read | | Finds a tool not in the list, an app's or Ghost's own | no |

## Remembering

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `recall` **core** | read | | Searches what was said before: your own conversations; Ghost searches any agent's | its own runs |
| `transcript_read` | read | | Pages back through recent turns | its own runs |
| `memory_read` | read | | An agent's memory, notes and persona; Ghost reads any agent's | no |

A transcript is what was said in a session or a run. What Ghost hears while it waits for
its name is never stored, so nothing here can find it.

## Working on a schedule

"Check the build every morning and tell me if it is red" is a schedule: an instruction an
agent carries out on its own at a time, with nobody there, its answer on your waiting list.

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `schedule_manage` **core** | change | quiet | Sets up, changes, pauses, resumes, removes or runs a schedule now | yes, its own |
| `schedules_list` | read | | The schedules, when each runs next, what the last runs said | yes |

## Watches

A watch polls one of an app's read-only tools on an interval, with no model, and tells
you, records, or has an agent act on what it finds ([folder.md](folder.md#watches)).

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `watches_list` | read | | The watches, their last check and item counts, who made each | yes |
| `watch_manage` | change | ask to make or change one; quiet to pause, resume or remove one the agent made | Writes `watches.json`, keeping your entries; refused whole when it would not pass `ghostd check` | no |

An agent manages the watches it made, Ghost any agent's; yours are yours alone. Webhooks
(`hooks.json`) are never written by an agent.

## Apps

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `apps_list` | read | | The connected apps and their state | yes |
| `app_tools` | read | | One app's tools | no |
| `apps_catalog_search` | read | | Finds an app in Ghost's copy of the catalog | no |
| `app_connect` | change | ask | Connects an app; one that needs your sign-in or a key is left on your waiting list for you | no |
| `app_disconnect` | change | ask | Disconnects an app | no |

## How Ghost reaches you, and quiet

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `routes_read` | read | | The routes that are on, quiet hours, do-not-disturb | yes |
| `quiet_until` | change | quiet | Do not disturb until a time or for a while, as "nothing until two" does | no |
| `quiet_off` | change | quiet | Ends do-not-disturb | no |

Which routes are on, and their order, stay yours: Settings › Notifications.

## Why Ghost asked before a change

When Ghost asked before closing an issue, or a change was blocked, your permissions said
so: the most careful of your `block`, `ask`, `quiet` and `allow` rules that names the call,
else the mode ([apps.md](apps.md#permissions)). An agent can read them and say which rule it
was; no agent can change them.

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `permissions_read` | read | | The mode and the rules, to explain why something was asked about or blocked | no |
| `settings_read` | read | | The settings that are not passwords, and the model in use | no |

## What Ghost made

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `output_read` | read | | Reads a file in `~/.config/ghost/out/`, a part at a time | no |
| `output_delete` | change | quiet | Deletes one | no |

Writing one is `outputs.write` and `outputs.replace` ([folder.md](folder.md#outputs)).

## Agents working together, and skills

| Tool | Kind | Default | What it does | Outside |
|---|---|---|---|---|
| `ask_agent` | change | quiet | Hands another agent a written task and returns its written answer; that agent cannot ask a third, and a session has three running at the most | no (it has its own `agent_ask`) |
| `skill_write` | change | ask | Writes a skill (a `SKILL.md` page, never a script) from what an agent learned | no |

## What no tool does

No tool changes a permission, reads a password, writes `policy.toml` or `hooks.json`, or
returns anything Ghost heard while waiting for its name. A rule in your `[permissions]` can
take any of these tools away (`block = ["ghost.app_connect"]`), ask first
(`ask = ["ghost.quiet_until"]`) or have Ghost say each time (`allow = ["ghost.notify"]`).

Besides Ghost's own app, an agent has the session's tools (`hang_up`, `hold`,
`switch_conversation`), the agents' (`list_agents`, `change` with `conversations.*`), the
rules' (`rules.add`, your words only), the skills' (`read_skill`, `skills.add`) and the
outputs' (`list_outputs`, `outputs.write`).
