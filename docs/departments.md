# Departments, toolkits and templates

> **Superseded by [ADR 0024](adr/0024-a-department-is-a-folder.md), accepted 2026-10-25,
> not yet built.** There a department is a folder under `~/.config/ghost/departments/`
> with its own `AGENTS.md`, `mcp.json`, `department.json`, skills, docs and shared
> `memory.md`; agents are hired into it, moved between departments and retired; toolkits
> and templates go. This page describes what runs today and is rewritten with the code.

How Ghost is organised above the servers and skills (ADR 0023). Five levels, each only
narrowing the one above: standards (servers, skills), **toolkits**, **departments**,
**lines**, and Ghost.

## Toolkits: what a department may reach

A toolkit is a named bundle of permissions in the policy file, designed in the desktop's
Toolkits screen:

```toml
[toolkit.finance]
description = "The books and the invoices."
skills = ["month-close", "invoicing"]

[toolkit.finance.servers.github_heka]
reads = ["issue_read", "list_issues"]     # optional: bare tool names; else every read
writes = ["comment", "set_labels"]        # optional: operations; else every declared one
items = ["heka-vault/ghost"]              # optional: a subset of the server's scope items
levels = { set_labels = "confirm" }       # optional: tighter than the file, never looser
```

The policy file is the ceiling. A toolkit may leave out a server, a tool, an item, and may
lower a level; it can never add or raise. A toolkit that tries is a problem `ghostd check`
and the Toolkits screen name. `GET /toolkits` lists them with their problems; the designer
saves through `PUT /toolkits/<name>`.

## Departments: lines with toolkits

A department is a line (ADR 0014) that has toolkits. It is its own primary line: "transfer
me to accounting" lands on it. Other lines can be started **in** it ("a line about the Q3
audit in accounting"): a topic line reaches what the department reaches, speaks as the
department unless it has its own persona, and keeps its own memory, notes and scope.

A session in a department gets only its toolkits' servers (narrowed to the granted reads,
writes and items) and skills: the rest are not in the prompt, not in the tool list, not
callable. A department with **no toolkits reaches everything**, like Ghost; give it
toolkits to narrow it. Ghost, line zero, always reaches everything and is where routing
starts: a request about an item some department's toolkit grants goes to that department
when department routing is on (ADR 0019).

A transfer swaps the system prompt and the tool list together; the first model call after
it writes the prompt cache again, which the latency line shows.

By voice: `conversations.create` takes `department` (start a topic line in one) and
`toolkits` (a new department's); `conversations.set_toolkits` changes a department's, at
`confirm`. In the desktop: the New line dialog and the line's settings.

## Templates: a department in one step

A template is a folder that creates a department with its toolkits and skills:

```
accounting/
├── department.toml     # title, aliases, voice, toolkits, [toolkit.*] definitions
├── persona.md
└── skills/month-close/SKILL.md
```

Ghost ships three under `infra/templates/` (triage, design-research, accounting); the
operator's own live under `~/.config/ghost/templates/` and win on a name clash. Importing
in the desktop (`POST /templates/<name>/apply`) adds the template's toolkits to the policy
file, copies its skills into Ghost's skills, and creates the department line, each step
audited. Exporting a department (`POST /conversations/<slug>/export`) writes the folder
back. By voice, "start an accounting department from the template" is
`conversations.create` with `template = "accounting"` at `confirm`: it creates the line with
the template's persona, voice, aliases and toolkits, and tells the operator to import in the
desktop first when the toolkits are not in the policy file yet, since the model never
writes policy.

## Scripts: run only through the gate

A skill's scripts never run from its text. They run through `ghost-scripts`, an MCP server
Ghost ships, declared like any server and scoped by skill:

```toml
[mcp.scripts]
command = "ghost-scripts"
args = ["--skills", "~/.config/ghost/skills"]
scope = { kind = "skill", arguments = ["skill"], items = ["month-close"] }

[mcp.scripts.writes.run]
target = ["skill", "script"]
asked_by = ["run"]
risk = "high"
bound = { args = 12 }

[mcp.scripts.levels]
run = "confirm"
```

A toolkit's `items` on it say whose scripts a department may run. A run gets a minimal
environment (no secret unless the server's `env` names it), its skill's folder as the
working directory, a 60 s timeout and a 20,000-character output cap; the audit row keeps
the arguments and the outcome. The operator may drop this gate later if it proves slow or
asks too often; that is a measured decision, not a default.
