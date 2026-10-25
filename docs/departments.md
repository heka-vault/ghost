# Departments and agents

A department is a folder; an agent is hired into it and can be moved to another (ADR
0024). Ghost is a department too: the config folder itself, where every wake lands. The
words the operator sees are **agent** and **department**; the code and the control API
keep *conversation* for an agent (`ghost_core.conversations`, `/conversations`).

```
~/.config/ghost/                    # Ghost: the department every wake lands in
├── AGENTS.md                       # Ghost's standing instructions, in every prompt
├── mcp.json                        # Ghost's servers
├── department.json                 # what the gate needs: reads, scope, writes, levels
├── memory.md                       # what every agent in Ghost knows (gate-written)
├── skills/<name>/SKILL.md
├── agents/ghost/                   # agent zero: agent.json, persona.md, memory.md, notes.md
└── departments/
    └── accounting/
        ├── AGENTS.md               # the department's rules, in every agent's prompt
        ├── mcp.json                # its own servers, or "use" for Ghost's
        ├── department.json         # title, aliases, voice, root, declarations and levels
        ├── memory.md               # the department's shared memory, gate-written
        ├── skills/<name>/SKILL.md
        ├── docs/…                  # read on demand
        └── agents/<slug>/          # agent.json, persona.md, memory.md, notes.md
```

The operator edits the folder with any editor; Ghost reads it (`ghost_core.departments`)
and writes into it only through the gate: an agent's files, a department's `memory.md`,
and a rule appended verbatim to `AGENTS.md`. The model never writes `mcp.json` or
`department.json`, so the folder is the permission ceiling for everything in it.

## A department's folder

- **`AGENTS.md`** is the department's rules. Every agent in it gets the text in its prompt,
  after Ghost's own `AGENTS.md` (so Ghost's rules apply everywhere). "Accounting, add a
  rule: always say the currency" appends the operator's words as they were said
  (`departments.add_rule`, medium risk, so `confirm` unless the policy file says
  otherwise). Capped at 8,000 characters in the prompt.
- **`mcp.json`** is its servers, in the shape other clients share: `{"mcpServers":
  {"<name>": {"command", "args", "env"}}}` for stdio, `{"type": "http", "url", "headers"}`
  for streamable HTTP, secrets as `${NAME}`. `"use": ["github"]` reaches one of Ghost's
  servers instead of starting another process. See [mcp.md](mcp.md).
- **`department.json`** is what the gate needs: `title`, `aliases` (other names the
  operator says for it), `voice` (the default for its agents), `root`, and `servers`, keyed
  by server name, each with the `reads`, `scope`, `writes` and `levels` of ADR 0022,
  unchanged in meaning. A write a server does not declare here is never offered.
- **`memory.md`** is what every agent in the department knows: short, in every prompt,
  written through `departments.remember` (low risk, announce). An agent's own `memory.md`
  stays private to it.
- **`skills/`** are the department's own skills, nearest first ([skills.md](skills.md));
  **`docs/`** are files the model reads on demand through `read_department_doc`.
- **`root`** (optional) points the department at a repository: its `AGENTS.md`, `mcp.json`
  or `.mcp.json`, skills and `docs/` are read there in place, so a project's own folder is
  the department's. The agents stay under `~/.config/ghost`. Ghost does not append to a
  rooted department's files.

Ghost's own folder is the config root. Its `department.json` carries the declarations for
Ghost's servers. A department that `use`s one of them reaches it through its own
declaration: its own `reads`, `scope` and `writes` for that server, and its own `levels`
and `targets`, which `Department.policy` lays over the policy file's `[defaults]` for a
session in that department. Both files are the operator's; the model writes neither.

## Agents

An agent is a conversation (ADR 0014) that works in a department: `agent.json` (its job,
voice, department), `persona.md`, a private `memory.md`, `notes.md`, and its transcript in
SQLite. Every department has a default agent, named after it, that answers when the
operator says the department's name; Ghost's is agent zero.

By voice, through the gate (`[conversations]` in the policy file):

- **Hire**: "hire an agent called Ledger in accounting" is `conversations.create`, with a
  persona Ghost drafts from the conversation and the operator edits (ADR 0019).
- **Move**: "move Ledger to legal" is `conversations.move`: the folder moves, the
  transcript and memory go with it, and a running session in that agent follows ("Moved to
  Legal."). A department's default agent does not move; Ghost offers to hire one in the
  destination with a summary of its memory instead.
- **Retire** and **rehire**: out of voice routing and the active list, transcript and audit
  kept; `conversations.delete` removes the folder and the rows for good.
- **Rename**, **set the job**, **set the voice** ("use the department's" clears it),
  **set the persona** (the operator's words verbatim), remember, forget, compact, reset.

In the desktop: Agents (grouped by department, with a Move dialog) and Departments (rules,
memory, servers, skills, docs, agents, journal, Add rule, New department, Open folder).

## A session in a department

Every session opens in Ghost. "Transfer me to accounting" or an agent's name moves it: the
prompt becomes that agent's (Ghost's rules, the department's rules and shared memory, the
agent's persona and memory, the roster of agents it can hand off to), the voice changes,
and the tools become what the department reaches: its own servers and the ones it `use`s,
its skills, its docs, and the agents' tools. Nothing else is in the prompt or callable. A
department with no servers reaches nothing outside; Ghost reaches its own servers.

A department's own servers start on the first transfer into it and stay up for
`department_idle_secs` (600) after the last session leaves (`ghost_core.departments.Fleet`);
the start is measured and logged with the transfer, since it is a cost on that turn
([performance.md](performance.md)). Ghost's servers connect at start. A server that fails
to connect is a problem the department reports (`ghostd check`, `GET /departments`, the
Departments screen) and is skipped.

A request about an item another department owns (a repository in its allowlist) gets a
tool result that names the owner, and Ghost offers to transfer; with
`GHOST_DEPARTMENT_ROUTING=1` it transfers and hands the request over.

## The journal

Every write in a department, by any of its agents, is in the audit with the agent and the
department on the row. `GET /departments/<slug>/journal` and the Departments screen show
a department's own, newest first.

## Shipped departments

`infra/templates/` holds three folders in this shape (`triage`, `accounting`,
`design-research`). Copy one under `~/.config/ghost/departments/` and edit it; nothing
there runs until it is copied.

## Scripts: run only through the gate

A skill's scripts never run from its text. They run through `ghost-scripts`, an MCP server
Ghost ships, declared like any server (in a department's `mcp.json` and `department.json`)
and scoped by skill, so the `items` of its scope say whose scripts may run:

```json
"scripts": {
  "scope": {"kind": "skill", "arguments": ["skill"], "items": ["month-close"]},
  "writes": {
    "run": {"tool": "run", "target": ["skill", "script"], "asked_by": ["run"],
            "risk": "high", "bound": {"args": 12}}
  },
  "levels": {"run": "confirm"}
}
```

A run gets a minimal environment (no secret unless the server's `env` names it), its
skill's folder as the working directory, a 60 s timeout and a 20,000-character output cap;
the audit row keeps the arguments and the outcome.
