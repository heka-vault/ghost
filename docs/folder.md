# Ghost's folder: rules, agents, watches, webhooks, outputs

Everything the operator edits lives in one folder, `~/.config/ghost/` (ADR 0035; there are
no departments). The words the operator sees are **agent**, **app** and **permission**; the
code and the control API keep *conversation* for an agent (`ghost_core.conversations`,
`/conversations`).

```
~/.config/ghost/
├── AGENTS.md          # rules for every agent, as the operator wrote them
├── apps.json          # the apps connected to Ghost (ADR 0034)
├── watches.json       # what Ghost watches (ADR 0028)
├── hooks.json         # the webhooks it takes (ADR 0027); fixtures in hooks/<name>/
├── skills/<name>/SKILL.md
├── skills/.skill-lock.json  # where the skills installed from GitHub came from (skills.md)
├── out/               # what Ghost makes (ADR 0033)
├── agents/<slug>/     # agent.json, persona.md, memory.md, notes.md (ghost: agent zero)
└── policy.toml, settings.env   # settings.env holds no secret; passwords are in Ghost's vault (ADR 0036)
```

The operator edits the folder with any editor; Ghost writes into it only through the gate:
an agent's files, a file in `out/`, and a rule appended verbatim to `AGENTS.md`. The model
never writes `apps.json`, `watches.json`, `hooks.json` or `policy.toml`. Every agent reaches
every connected app, less what the operator's permissions block ([apps.md](apps.md)); there
is one set of permissions and nothing narrows it per agent.

Files from before ADR 0035 are not read and not migrated: `ghostd check` names a
`department.json` at the root (move its `watches` and `hooks` into `watches.json` and
`hooks.json` by hand), a `memory.md` or `docs/` at the root, and anything under
`departments/`.

## Rules

`AGENTS.md` is in every agent's prompt, as written. "Add a rule: always say the currency"
appends the text as the operator said it (`rules.add`; it runs and Ghost says so, unless
the permissions ask first). By voice the text must be in the operator's request word for
word (case, punctuation and spacing aside) or the gate refuses it. With
`ask = ["rules.add"]` the request is never the yes for this one: Ghost reads the text back
and the spoken yes writes it. In the desktop the operator types the rule on the Agents
screen. A linked `AGENTS.md` is a problem the folder reports; Ghost reads and writes only
regular files. A rule edited by hand applies from the next session.

## Agents

An agent is a conversation (ADR 0014) with a name, a job, a persona and a voice:
`agent.json`, `persona.md`, a private `memory.md`, `notes.md`, and its transcript in
SQLite. Ghost is agent zero. An agent without its own voice uses Ghost's.

By voice, through the gate (`conversations.*` rules in the permissions):

- **Hire**: "hire an agent called Ledger to keep the books" is `conversations.create`, with
  a persona Ghost drafts from the conversation and the operator edits (ADR 0019).
- **Retire** and **rehire**: out of voice routing and the active list, transcript and audit
  kept; `conversations.delete` removes the folder and the rows for good.
- **Rename**, **set the job**, **set the voice** ("use Ghost's" clears it), **set the
  persona** (the operator's words verbatim), remember, forget, compact, reset.

Every session opens in Ghost. "Transfer me to Ledger" names an agent: the prompt becomes
that agent's (the rules, its persona and memory) and the voice changes; the tools stay
Ghost's. A call straight into an agent from the phone or the desktop opens there. Apps
connect when the daemon starts, so a transfer starts nothing.

In the desktop, the Agents screen lists them, with the rules, Add rule, Open folder, and
what Ghost made. The Audit log shows every change and the agent that made it.

## Watches

Ghost can watch an outside system (ADR 0028): `watches.json`, one object per watch, keyed
by its name, written by the operator, never by the model. The daemon calls the read
on an interval itself; nothing about noticing needs a model.

```json
{
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
    "key": "{id}",
    "title": "CI failed on {branch}: {name}",
    "link": "{url}",
    "response": "tell"
  }
}
```

- **`app` and `read`**: a connected app and one of its read-only tools. The permissions'
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
- **`key`, `title`, `link`**: `{field}` templates, nothing else. Ghost puts the watch's
  name in front of the key (`{id}` is `ci:123`). The key makes one failure one item
  however often it is seen; a key that is gone next time is resolved. A look
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
`ghostd check` lists the watches, `GET /watches` and the desktop's Watching screen show each
one's last look, error and open items, and a watch that doesn't read is a problem and stays
off. A watch's items are keyed `<watch>:<key>` (`ci:123`), and `[routes.urgency]` names it
bare (`ci = "call"`).

## Webhooks

Ghost can take webhooks (ADR 0027): `hooks.json`, keyed by name. The daemon serves
`POST /hooks/<name>` on `GHOST_HOOKS_LISTEN` (off when empty; its own
addresses, nothing else on them). Every step before an item is deterministic and tested:

```json
{
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
        "key": "{repo}",
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
  five-minute window, and `X-Ghost-Event` names the event. The secret is `${NAME}`, a
  password in Ghost's vault (ADR 0036); a hook whose secret isn't set stays off, and an empty one verifies
  nothing.
- **`events`**: only declared events count; anything else is ignored. Each event needs
  `when` or `resolve` (or both): they say which deliveries count, and an event with
  neither turns the whole hook off. `when` checks open the item, `resolve` checks close
  it, and a delivery that matches neither is ignored. Fields are typed, constrained
  paths, as a watch's are; one that breaks its constraint refuses the whole delivery, as
  does a missing field the `key` or `title` names, and everything not declared is
  dropped.
- **`check`**: the reference is read again through one of the app's read-only tools, the
  permissions' `block` rules applied, and that read is believed, for a resolve as much as
  for an item: an item counts only when the check's `when` holds, and a resolve only when
  it no longer does (so an event with `resolve` and `check` must give `check` a `when`).
  A read that doesn't agree, fails or is blocked refuses the delivery but doesn't use up
  its delivery id, so the sender's retry (GitHub's Redeliver, after an API that lagged its
  webhook) is heard; a retry is read again and passes only if the source agrees then.
  Without `check` the item says unchecked, and a resolve is taken as sent. A hook closes
  only its own items, never a watch's under the same key.
- **Fixtures**: recorded deliveries in `hooks/<name>/*.json` beside `hooks.json`, each
  with `headers`, `body`, an `expect` (`item` with its `key`/`title`/`fields`, `resolved`,
  `ignored`, or `rejected` with a `reason`), and optionally `sign` (`bad`, `none`),
  `age_secs`, `replay`, and `readback` (what the source's read returns; `{"refuse": true}`
  for a read that is refused). They are signed with a fixture secret, never a real one. A
  hook with no fixtures, or with one that fails, stays off; `ghostd check` runs them.

What is kept is the parsed fields, the delivery id and the outcome, never a body. The
listener answers 404 for a hook that isn't declared (or is off) before it reads the body,
and accepted or refused otherwise, nothing more; `GET /watches` and the Watching screen
show each hook's counts. A local sender signs with `ghostd push <hook> <event> '<json>'`.

## Outputs

What Ghost makes (ADR 0033), a page, a summary, a table, a small script, lands in
`~/.config/ghost/out/`. `outputs.write` creates a file and `outputs.replace` replaces one,
each through the gate with an audit row; both run and Ghost says so unless the permissions
ask first (`ask = ["outputs.replace"]`). A name is a plain file name ending in `.html`,
`.md`, `.txt`, `.csv`, `.json`, `.svg`, `.py` or `.sh`; nothing goes through a link; a file
is at most 256 KB and is never made executable. Ghost never runs what it makes. The model
sees the list with `list_outputs`; the desktop's Agents screen lists them with Reveal and
Open.

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
