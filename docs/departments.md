# Departments and agents

A department is a folder; an agent is hired into it and can be moved to another (ADR
0024). Ghost is a department too: the config folder itself, where every wake lands. The
words the operator sees are **agent** and **department**; the code and the control API
keep *conversation* for an agent (`ghost_core.conversations`, `/conversations`).

```
~/.config/ghost/                    # Ghost: the department every wake lands in
├── AGENTS.md                       # Ghost's standing instructions, in every prompt
├── apps.json                       # the apps connected to Ghost (ADR 0034)
├── department.json                 # title, voice, watches, hooks
├── memory.md                       # what every agent in Ghost knows (gate-written)
├── skills/<name>/SKILL.md
├── agents/ghost/                   # agent zero: agent.json, persona.md, memory.md, notes.md
└── departments/
    └── accounting/
        ├── AGENTS.md               # the department's rules, in every agent's prompt
        ├── department.json         # title, aliases, voice, root, permissions, watches
        ├── memory.md               # the department's shared memory, gate-written
        ├── skills/<name>/SKILL.md
        ├── docs/…                  # read on demand
        └── agents/<slug>/          # agent.json, persona.md, memory.md, notes.md
```

The operator edits the folder with any editor; Ghost reads it (`ghost_core.departments`)
and writes into it only through the gate: an agent's files, a department's `memory.md`,
and a rule appended verbatim to `AGENTS.md`. The model never writes `department.json` or
`apps.json`. Apps are Ghost's, connected once and reached from every department; a
department's `permissions` only ever add care over the operator's own ([apps.md](apps.md)).

## A department's folder

- **`AGENTS.md`** is the department's rules. Every agent in it gets the text in its prompt;
  Ghost's own `AGENTS.md` is Ghost's and is not repeated in other departments. "Accounting,
  add a rule: always say the currency" appends the text as the operator said it
  (`departments.add_rule`; it runs and Ghost says so, unless the permissions ask first).
  By voice the text must be in the operator's request word for word (case, punctuation
  and spacing aside) or the gate refuses it. With `ask = ["departments.add_rule"]` the
  request is never the yes for this one: Ghost reads the text back and the spoken yes
  writes it. In the desktop the operator types the rule. Capped at 8,000 characters in the prompt. A linked `AGENTS.md` or
  `memory.md` is a problem the folder reports; Ghost reads and writes only regular files.
- **`department.json`**: `title`, `aliases` (other names the operator says for it),
  `voice` (the default for its agents), `root`, `permissions`, `watches` and `hooks`.
  `permissions` holds `block`, `ask` and `quiet` rules for its own agents' calls, in the
  syntax of [apps.md](apps.md#permissions): `{"block": ["github.*(repo!=site)",
  "stripe"], "ask": ["github.issue_write(state=closed)"]}`. It cannot `allow`. An older
  folder's `servers` and `mcp.json` are not read any more and are named as a problem
  (ADR 0034).
- **`memory.md`** is what every agent in the department knows: short, in every prompt,
  written through `departments.remember` (low risk, announce). An agent's own `memory.md`
  stays private to it.
- **`skills/`** are the department's own skills, nearest first ([skills.md](skills.md));
  **`docs/`** are files the model reads on demand through `read_department_doc`.
- **`root`** (optional) points the department at a repository: its `AGENTS.md`, skills
  and `docs/` are read there in place, so a project's own folder is the department's. The
  repository's own `.mcp.json` is its coding tools' business and is not read. The agents
  stay under `~/.config/ghost`. Ghost does not append to a rooted department's files.

Ghost's own folder is the config root; its permissions are the policy file's, and a
`permissions` in its `department.json` is a problem. A department's rules are added to the
operator's for a session in that department (`Department.policy`), and the most careful
rule that matches wins, so a department never widens what Ghost may do. A rule edited by
hand applies at the next refresh of the folders (the desktop's Departments screen, a
policy-file reload, a daemon restart).

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
memory, permissions, skills, docs, agents, journal, Add rule, New department, Open folder).

## A session in a department

Every session opens in Ghost. "Transfer me to accounting" or an agent's name moves it: the
prompt becomes that agent's (the department's rules and shared memory, the agent's
persona and memory), the voice changes, and the tools become what the department reaches:
every connected app less what its permissions block, its skills, its own docs and journal,
and the agents' tools. A department's session cannot read or write another department's
files (only Ghost can, and Ghost alone carries the roster of departments and agents). A
call straight into an agent from the phone or the desktop opens in its department the
same way. Apps connect when the daemon starts, so a transfer starts nothing.

## Watches

A department can watch an outside system (ADR 0028): `watches` in `department.json`, one
object per watch, written by the operator, never by the model. The daemon calls the read
on an interval itself; nothing about noticing needs a model.

```json
"watches": {
  "ci": {
    "app": "github",
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

- **`app` and `read`**: a connected app and one of its read-only tools. The department's
  `block` rules apply as for the model's reads; a change is never a watch's read. The
  result is not trimmed for speech, but is capped. (`server` is the older word; it still
  reads.)
- **`fields`**: what to keep from each result (`items` is the path to the list), each a
  dotted path with a `type` (`str`, `int`, `float`, `bool`) and optionally a `pattern`, a
  `max` length or an `enum`. A result whose field breaks its constraint, or that lacks a
  field the `key` or `title` names, is dropped whole; everything else the app sent is
  never kept. A result with no list at `items` is a failed read, not an empty one.
- **`when`**: checks over the fields (`==`, `!=`, `in`, `not_in`, `>`, `>=`, `<`, `<=`,
  `exists`), all of which must hold; `for_minutes` waits until a result has held that long
  before it becomes an item. An item already open stays open while it matches, so a
  restart (which starts the wait again) doesn't resolve it and tell you again later.
- **`key`, `title`, `link`**: `{field}` templates, nothing else. The key makes one failure
  one item however often it is seen; a key that is gone next time is resolved. A look
  that dropped results (one broke a constraint, or there were more than 100) resolves
  nothing, since a dropped result may be the open one; a failed read resolves nothing.
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
  couldn't say. An answer is remembered while the item's fields stay the same. When a
  question that could be asked isn't answered (Jev fails, or the poll's ten questions are
  spent), an item already open keeps the response it has rather than taking the default.

An app whose connection died is started again before the next look, backing off while it
fails; so does a look that breaks for any other reason, with the error on the watch.
`ghostd check` lists the watches, `GET /departments` shows each one's last look, error and
open items, and a watch that doesn't read is a problem on the department and stays off.

## Webhooks

A department can take webhooks (ADR 0027): `hooks` in `department.json`. The daemon serves
`POST /hooks/<department>/<name>` on `GHOST_HOOKS_LISTEN` (off when empty; its own
addresses, nothing else on them). Every step before an item is deterministic and tested:

```json
"hooks": {
  "ci": {
    "verify": "github",
    "secret": "${GITHUB_HOOK_SECRET}",
    "events": {
      "workflow_run": {
        "fields": {
          "action": {"at": "action"},
          "conclusion": {"at": "workflow_run.conclusion"},
          "id": {"at": "workflow_run.id", "type": "int"},
          "repo": {"at": "repository.full_name", "pattern": "^[\\w.-]+/[\\w.-]+$"}
        },
        "when": [{"field": "conclusion", "op": "==", "value": "failure"}],
        "resolve": [{"field": "conclusion", "op": "==", "value": "success"}],
        "key": "ci:{repo}",
        "title": "CI failed on {repo}",
        "check": {
          "app": "github", "read": "get_workflow_run",
          "args": {"owner": "heka-vault", "repo": "ghost", "run_id": "{id}"},
          "fields": {"conclusion": {"at": "conclusion"}},
          "when": [{"field": "conclusion", "op": "==", "value": "failure"}]
        }
      }
    }
  }
}
```

- **`verify`**: `github` checks `X-Hub-Signature-256` over the raw body and refuses a
  repeated `X-GitHub-Delivery`. GitHub signs only the body, not the delivery id, so a body
  seen before is refused too, whatever id it carries (its hash is kept beside the
  delivery, never the body). `hmac` is Ghost's own scheme for local senders:
  `X-Ghost-Signature: sha256=` HMAC of `<X-Ghost-Timestamp>.<X-Ghost-Delivery>.<body>`, a
  five-minute window, and `X-Ghost-Event` names the event. The secret is `${NAME}` from
  the environment; a hook whose secret isn't set stays off, and an empty one verifies
  nothing.
- **`events`**: only declared events count; anything else is ignored. Each event needs
  `when` or `resolve` (or both): they say which deliveries count, and an event with
  neither turns the whole hook off. `when` checks open the item, `resolve` checks close
  it, and a delivery that matches neither is ignored. Fields are typed, constrained
  paths, as a watch's are; one that breaks its constraint refuses the whole delivery, as
  does a missing field the `key` or `title` names, and everything not declared is
  dropped.
- **`check`**: the reference is read again through one of the app's read-only tools, the
  department's `block` rules applied, and that read is believed, for a resolve as much as
  for an item: an item counts only when the check's `when` holds, and a resolve only when
  it no longer does (so an event with `resolve` and `check` must give `check` a `when`).
  A read that doesn't agree, fails or is blocked refuses the delivery but doesn't use up
  its delivery id, so the sender's retry (GitHub's Redeliver, after an API that lagged its
  webhook) is heard; a retry is read again and passes only if the source agrees then. Without `check` the item says unchecked, and a resolve is taken as sent. A hook
  closes only its own items, never a watch's under the same key.
- **Fixtures**: recorded deliveries in `hooks/<name>/*.json` beside `department.json`, each
  with `headers`, `body`, an `expect` (`item` with its `key`/`title`/`fields`, `resolved`,
  `ignored`, or `rejected` with a `reason`), and optionally `sign` (`bad`, `none`),
  `age_secs`, `replay`, and `readback` (what the source's read returns; `{"refuse": true}`
  for a read that is refused). They are signed with a fixture secret, never a real one. A
  hook with no fixtures, or with one that fails, stays off; `ghostd check` runs them.

What is kept is the parsed fields, the delivery id and the outcome, never a body. The
listener answers 404 for a hook that isn't declared (or is off) before it reads the body,
and accepted or refused otherwise, nothing more; `GET /departments` shows each hook's
counts. A local sender signs with `ghostd push <department>/<hook> <event> '<json>'`.

## Outputs

What Ghost makes (ADR 0033), a page, a summary, a table, a small script, lands in the
department's `out/` folder (Ghost's is `~/.config/ghost/out/`), never in a `root`.
`outputs.write` creates a file and `outputs.replace` replaces one, each through the gate
with an audit row; both run and Ghost says so unless the permissions ask first
(`ask = ["outputs.replace"]`). A name is a plain file name
ending in `.html`, `.md`, `.txt`, `.csv`, `.json`, `.svg`, `.py` or `.sh`; nothing goes
through a link; a file is at most 256 KB and is never made executable. Ghost never runs
what it makes. The model sees the list with `list_outputs`; the desktop's department page
lists them with Reveal and Open.

## The journal

Every write in a department, by any of its agents, is in the audit with the agent and the
department on the row. `GET /departments/<slug>/journal` and the Departments screen show
a department's own, newest first.

## Shipped departments

`infra/templates/` holds three folders in this shape (`triage`, `accounting`,
`design-research`). Copy one under `~/.config/ghost/departments/` and edit it; nothing
there runs until it is copied.

## Scripts: run only through the gate

A skill's scripts never run from its text. They run through `ghost-scripts`, an app Ghost
ships, connected in `apps.json` like any app that runs on this computer, with the skills
folder it may run from (`--skills`) and the folders a script may work in (`--root`).
`run` is a change like any other: it runs and Ghost says so, so a careful operator writes
`ask = ["scripts.run"]`, or keeps it to some skills with
`block = ["scripts.run(skill!=month-close)"]`.

A run gets a minimal environment (no secret unless the app's `env` names it), its
skill's folder as the working directory, a 60 s timeout and a 20,000-character output cap;
the audit row keeps the arguments and the outcome.
