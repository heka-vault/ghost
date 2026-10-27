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

- **`AGENTS.md`** is the department's rules. Every agent in it gets the text in its prompt;
  Ghost's own `AGENTS.md` is Ghost's and is not repeated in other departments. "Accounting,
  add a rule: always say the currency" appends the text as the operator said it
  (`departments.add_rule`, medium risk, `confirm` unless the policy file says otherwise).
  By voice the text must be in the operator's request word for word (case, punctuation
  and spacing aside) or the gate refuses it; the request is never the yes for this one,
  Ghost reads the text back and the spoken yes writes it. In the desktop the operator
  types the rule. Capped at 8,000 characters in the prompt. A linked `AGENTS.md` or
  `memory.md` is a problem the folder reports; Ghost reads and writes only regular files.
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
- **`root`** (optional) points the department at a repository: its `AGENTS.md`, skills
  and `docs/` are read there in place, so a project's own folder is the department's. The
  repository's `mcp.json` or `.mcp.json` names commands to run and the secrets to hand
  them, so it starts nothing until `department.json` also says `"root_servers": true`;
  until then the department's own `mcp.json` is read and the root's is listed as a
  problem. The agents stay under `~/.config/ghost`. Ghost does not append to a rooted
  department's files.

Ghost's own folder is the config root. Its `department.json` carries the declarations for
Ghost's servers. A department that `use`s one of them reaches it through its own
declaration: its own `reads` and `writes` for that server, and its own `levels` and
`targets`, which `Department.policy` lays over the policy file's `[defaults]` for a session
in that department. Its allowlist never widens Ghost's: the scope's kind and arguments are always Ghost's,
no `items` of its own means Ghost's items, and an item Ghost does not allow is dropped and
reported as a problem. The gate keeps each department's operations apart, so a used
server's pinned arguments, bounds and allowlist are the department's own in its sessions
and Ghost's in Ghost's. Both files are the operator's; the model writes neither. A level
or a declaration edited by hand applies at the next refresh of the folders (the desktop's
Departments screen, a policy-file reload, a daemon restart): the levels are rebuilt and
every department's operations are registered again, and a department never runs Ghost's
operation in place of one its own declaration lacks.

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
prompt becomes that agent's (the department's rules and shared memory, the agent's
persona and memory), the voice changes, and the tools become what the department reaches:
its own servers and the ones it `use`s, its skills, its own docs and journal, and the
agents' tools. Nothing else is in the prompt or callable; a department's session cannot
read or write another department's files (only Ghost, unscoped, can, and Ghost alone
carries the roster of departments and agents). A department with no servers reaches
nothing outside; Ghost reaches its own servers. A call straight into an agent from the
phone or the desktop enters its department the same way before the session is built.

A department's own servers start on the first transfer into it and stay up for
`department_idle_secs` (600) after the last session leaves (`ghost_core.departments.Fleet`);
the start is measured and logged with the transfer, since it is a cost on that turn
([performance.md](performance.md)). Ghost's servers connect at start. A server that fails
to connect is a problem the department reports (`ghostd check`, `GET /departments`, the
Departments screen) and is skipped; a start that brings nothing up is tried again at the
next transfer.

A request about an item another department owns (a repository in its allowlist) gets a
tool result that names the owner, and Ghost offers to transfer; with
`GHOST_DEPARTMENT_ROUTING=1` it transfers and hands the request over.

## Watches

A department can watch an outside system (ADR 0028): `watches` in `department.json`, one
object per watch, written by the operator, never by the model. The daemon calls the read
on an interval itself; nothing about noticing needs a model.

```json
"watches": {
  "ci": {
    "server": "github",
    "read": "list_workflow_runs",
    "args": {"owner": "heka-vault", "repo": "ghost", "status": "failure"},
    "every": 60,
    "items": "workflow_runs",
    "fields": {
      "id": {"at": "id", "type": "int"},
      "branch": {"at": "head_branch", "pattern": "^[\\w./-]+$"},
      "name": {"at": "name", "max": 120},
      "url": {"at": "html_url", "pattern": "^https://github\\.com/.*"}
    },
    "when": [{"field": "branch", "op": "==", "value": "main"}],
    "key": "ci:{id}",
    "title": "CI failed on {branch}: {name}",
    "link": "{url}",
    "response": "tell"
  }
}
```

- **`server` and `read`**: a server the department starts or uses, and one of the reads
  its declaration offers. The allowlist applies as for the model's reads; a write is never
  a watch's read. The result is not trimmed for speech, but is capped.
- **`fields`**: what to keep from each result (`items` is the path to the list), each a
  dotted path with a `type` (`str`, `int`, `float`, `bool`) and optionally a `pattern`, a
  `max` length or an `enum`. A result whose field breaks its constraint is dropped whole;
  everything else the server sent is never kept.
- **`when`**: checks over the fields (`==`, `!=`, `in`, `not_in`, `>`, `>=`, `<`, `<=`,
  `exists`), all of which must hold; `for_minutes` waits until a result has held that long.
- **`key`, `title`, `link`**: `{field}` templates, nothing else. The key makes one failure
  one item however often it is seen; a key that is gone next time is resolved.
- **`response`**: `tell` puts the item on the waiting list, `record` keeps it without
  alerting. `urgency` (`digest`, `notify`, `call`) is for the route policy (ADR 0031).
- **`jev` and `model`** (optional): where the check can't decide, a typed question:

  ```json
  "jev": {
    "question": "Is this the known flaky end-to-end test, or a new failure?",
    "fields": ["name"],
    "choices": {"tell": "a new failure", "record": "the known flaky test"},
    "default": "tell"
  }
  ```

  `fields` is what may leave the machine for it; `choices` are two or more of `tell`,
  `record` and `drop`; the `default` applies when Jev is off, late, unsure, or answers
  anything else. `model` takes the same shape and is asked only when Jev isn't declared or
  couldn't say. An answer is remembered while the item's fields stay the same.

A department with a watch on its own servers keeps them up while the daemon runs; a server
whose transport died is started again before the next look, backing off while it fails.
`ghostd check` lists the watches, `GET /departments` shows each one's last look, error and
open items, and a watch that doesn't read is a problem on the department and stays off.

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
