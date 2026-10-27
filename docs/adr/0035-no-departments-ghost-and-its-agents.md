# 0035: No departments; Ghost and its agents

- **Status:** accepted (decided 2026-10-27)
- **Date:** 2026-10-27
- **Supersedes:** ADR 0024's departments: the department folder, its `department.json`,
  shared memory, `docs/`, journal, `root`, default agent, move, skills read nearest first,
  and a department's own permissions (ADR 0034's narrowing); ADR 0027 and ADR 0028 where they
  put watches and webhooks in a department folder; ADR 0033's `out/` per department
- **Keeps:** agents (ADR 0014, ADR 0024's hire, retire and rehire)

ADR 0024 made a department the operator's word for a project: a folder of servers, skills,
rules and memory with agents inside it. ADR 0034 then took the apps away from departments,
and what was left mostly repeated what an agent already has: a name, rules, a voice. Two
days later the operator had no department besides Ghost's own folder and one agent, Ghost,
while the layer cost a permission merge (which widened the operator's permissions, the QA
review of 2026-10-27 found), a department check on every pending change, isolation rules,
skills read nearest first, a screen, API routes and prompt tokens on every turn. The
operator's decision is to drop the layer and keep agents.

## Decisions

**There are no departments.** Ghost is one assistant with one folder,
`~/.config/ghost/`, and one set of permissions, `[permissions]` in `policy.toml`. Every
agent reaches every connected app, less what those permissions block. Nothing narrows them
per agent; if that is ever wanted, it is a field on an agent, not a layer.

**Agents stay, flat.** An agent is a folder under `~/.config/ghost/agents/<slug>/`
(`agent.json`, `persona.md`, `memory.md`, `notes.md`), hired, retired and rehired as
before. Ghost is agent zero. There is no move and no default agent per department;
"transfer me to Maya" names an agent. An agent without its own voice uses Ghost's.

**What Ghost's folder holds:**

```
~/.config/ghost/
├── AGENTS.md          # rules for every agent, as the operator wrote them
├── apps.json          # connected apps (ADR 0034)
├── watches.json       # what Ghost watches (ADR 0028)
├── hooks.json         # webhooks it takes (ADR 0027), fixtures in hooks/<name>/
├── skills/<name>/SKILL.md
├── out/               # what Ghost makes (ADR 0033)
├── agents/<slug>/
└── policy.toml, env, settings.env, apps.env
```

- **Rules.** `AGENTS.md` is in every agent's prompt. "Add a rule" appends the operator's
  words verbatim through the gate as `rules.add`, never a paraphrase (ADR 0014).
- **Skills** are Ghost's `skills/`, then the operator's `~/.agents/skills/`. There is no
  nearer folder to read first.
- **Watches and webhooks** are `watches.json` and `hooks.json`, keyed by name, in the shapes
  ADR 0027 and ADR 0028 gave them. A watch's items are keyed `<watch>:<key>`, its source is
  `watch:<name>`, a hook's is `hook:<name>`, the listener serves `POST /hooks/<name>`, and
  `[routes.urgency]` names them bare (`ci = "call"`).
- **Outputs** are one folder, `out/`.

**What goes:** a department's shared `memory.md` (an agent's own memory stays private, ADR
0014), `docs/` (a skill is the thing read on demand), the journal (the desktop's Audit log
shows every change and the agent that made it), `root`, the templates, aliases for a
department, a department's voice, and the operations `departments.add_rule`,
`departments.remember` and `conversations.move`.

**The product shows agents, never departments.** The desktop's Departments screen becomes
*Watching*: the watches and webhooks, each with its last look and its counts. The rules and
the outputs move to the Agents screen, where every agent's rules and work are. The phone
lists agents without grouping.

## Consequences

- `ghost_core.departments` and `ghost_core.department_tools` go. Rules become a small source
  (`rules`), watches and hooks load from Ghost's folder, `WriteGate.policy_for` has one
  policy, and `bring_back` has no department to check.
- The control API: `/departments` and its routes go, with `/conversations/{slug}/move`;
  `GET /rules` and `POST /rules`, `GET /outputs` and `GET /watches` replace what the
  Departments screen read. Views lose `department` and `is_default`. The database keeps its
  `department` columns, unread.
- Every turn's prompt is smaller: no roster of departments, no department tools.
- No migration, as in ADR 0024 and ADR 0034. `ghostd check` names what is no longer read:
  `department.json` (its `watches` and `hooks` go into `watches.json` and `hooks.json` by
  hand), `memory.md` and `docs/` at the root, and anything under `departments/`. Open
  waiting-list items keyed by department resolve on their own and come back under the new
  keys.

## Considered options

- **Keep departments, fix the merge.** Rejected: the fix is easy, but the layer stays
  unused and keeps costing on every turn.
- **Move each department's parts onto an agent** (its own permissions, a `root`). Rejected
  for now by the operator; either can come back as a field on an agent when one is needed.
- **Drop agents too.** Rejected: hire, persona, voice and private memory are cheap, and the
  phone's "open the agent a question belongs to" (#22) builds on them.
