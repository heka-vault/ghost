# 0023: Departments, toolkits and skills

- **Status:** accepted (decisions taken with the operator on 2026-10-20; built the same day: skills, toolkits, departments, templates, the scripts server)
- **Date:** 2026-10-20
- **Builds on:** ADR 0014 (lines), ADR 0019 (scope), ADR 0022 (MCP servers are core)

The operator wants Ghost organised the way a company is: an Accounting department reaches the
finance servers and skills and nothing about source code; each department has a primary
line and more lines per topic; "transfer me to accounting" and "send me over to the Q3 audit
conversation" move the operator around; departments are stamped out from templates and
managed in the desktop. Skills, the `SKILL.md` standard, join MCP servers as the second
kind of thing Ghost can be given.

## Decisions

**Five levels, each only narrowing the one above.**

| Level | What it is | Where it lives |
|---|---|---|
| Standards | MCP servers (ADR 0022) and skills: `SKILL.md` folders. Inventory, no permissions of their own. | servers in the policy file; skills in `~/.config/ghost/skills/<name>/` and, read in place, `<root>/.agents/skills/<name>/` under a department's project root |
| Toolkits | A named bundle of permissions: servers (optionally only some tools and some scope items, at a level no looser than the policy's), and skills. | `[toolkit.<name>]` in the policy file, designed in the desktop |
| Departments | Persona, voice, a compiled prompt, toolkits, an optional project root, its lines. | SQLite plus `~/.local/share/ghost/departments/<slug>/` |
| Lines | A conversation with a department about a topic, with its own transcript, memory, notes and a scope that may narrow the department's. One line per department is primary. | the `conversations` table, with `department` and `primary` |
| Ghost | Line zero: the switchboard. Unscoped, every server at the policy's levels, every skill. | as today |

**The policy file is the ceiling.** `~/.config/ghost/policy.toml`, the desktop's "What Ghost
may do", sets the level per server, operation and target. A toolkit may lower a level or
leave out a server, a tool, a scope item; it can never raise a level or add what the file
does not allow. A toolkit that tries is a configuration error named at start. The toolkit
designer is a desktop screen; the file is its storage, not its only editor.

**A department's reach is the union of its toolkits.** A session in one of its lines is built
with only those servers' tools and those skills: the rest are not in the prompt, not in the
tool list, not callable. Ghost keeps everything, so the operator always has a full view and
a place to route from.

**Memory is per line.** A department holds its persona and notes; each line keeps its own
transcript and memory (ADR 0014 unchanged).

**Skills are instructions the model reads on demand.** A department's prompt carries the
index (name and one line each); a `read_skill` tool returns the body, trimmed, and the files
beside it one at a time. Skill text is untrusted like issue text: it shapes what the model
does, never what it may do; it cannot name a server the toolkit does not grant.

**Scripts in skills run only through the gate, for now.** A skill's scripts run through one
MCP server Ghost ships (`ghost-scripts`, stdio), so a run is a declared operation: a level
in the policy file (`confirm` by default), a timeout and an output cap, an audit row with the
arguments and the output, and no secret in the script's environment unless the toolkit names
it. The toolkit says which skills' scripts may run; a script never runs because a skill's
text asked. The operator may drop this gate if it proves slow or asks too often; that is a
later decision, measured first.

**Department templates** (the operator's "workspaces", a word now dropped) are folders that
create a department with its toolkits and skills in one step: `department.toml`,
`persona.md`, `skills/`. Ghost ships a few; the desktop imports and exports them; "start an
accounting department from the template" works by voice at `confirm`. A template is never
live: it is copied into a department, which is then the operator's.

**Transfers resolve departments and lines alike.** A department name lands on its primary
line; a line name lands on that line wherever it is; department routing (ADR 0019) sends a
request about another department's item to that department's primary line. The phone's dial
pad groups by department.

**Management is desktop-only, and every write goes through the gate.** Skills (drag and
drop, add by path, remove), servers, toolkits, departments, templates: the desktop edits
them; by voice Ghost proposes the same changes as operations at `confirm`. Nothing here is
reachable by the model except through `change`.

## As built

- A department is a line with toolkits, not a new kind of record: `Profile.toolkits` and
  `Profile.department` (the parent of a topic line). The department's own line is its
  primary line. Older lines load as departments with no toolkits.
- A department with no toolkits reaches everything, like Ghost. Narrowing is opt-in, so
  nothing changed for existing lines on the day this landed.
- A session's reach is computed per department from the live policy
  (`ConversationRuntime.sources_for`); every read tool is registered once and dispatched
  to the active line's reach, so a transfer only swaps the advertised tools
  (`LLMSetToolsFrame`) and the prompt.
- By voice, a template creates the department; its toolkits must already be in the policy
  file (the desktop's import puts them there). The model never writes policy.
- The scripts server is scoped by skill, so a toolkit's `items` on it say whose scripts a
  department may run; `run` is a declared write at `confirm`.

## Consequences

- `ghost_core.skills` (discovery, index, `read_skill`, the `add` and `remove` operations),
  `[toolkit.<name>]` in `ghost_core.policy` validated against the ceiling, `ghost_core.departments`,
  `conversations` with `department` and `primary`, the audit row with `department` and
  `toolkit`, `build_session` built from a department's reach, a transfer that swaps prompt
  and tools, the compiled prompt stored per department.
- Control API: `/skills`, `/toolkits`, `/departments`; the desktop gains Skills, Toolkits
  and Departments screens and regroups Lines under departments; the phone's dial pad groups
  by department.
- Performance (ADR 0018): a department's prompt is compiled once and reused, so the prompt
  cache holds; the index of skills and servers has a token budget; a transfer's cost (one
  cache write) is measured and shown.
- Migration: on first start every existing line becomes a department with that line as
  primary and its scope as an inline toolkit; files, not just rows, so it can be undone.
- Order of work: skills; toolkits; departments with the migration and the transfer; templates;
  then the scripts server, measured before it stays.
- The glossary gains Toolkit, Department, Skill and Template; *ability* stays the retired
  packages; *workspace* is not a Ghost word.
