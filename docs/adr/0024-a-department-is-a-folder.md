# 0024: A department is a folder; agents are hired into it and move between them

- **Status:** accepted 2026-10-25 (reviews 2026-10-21, 22 and 25; the last against OpenBot, #12); servers per department (`mcp.json`, `use`, the declarations, `root_servers`) superseded by ADR 0034: apps are Ghost's, and a department narrows them with permissions
- **Date:** 2026-10-22
- **Supersedes:** ADR 0023's toolkits, templates and departments-as-lines; ADR 0022's servers
  in the policy file; ADR 0019's per-conversation scope; the words *line* and *toolkit*
  (ADR 0014, GLOSSARY.md)

The operator thinks of a department as a project: a folder he can open and edit, a layer
around a group of tools, skills, contacts and memories. Agents come and go from it. He hires
one for brand design and another for social media, both in marketing; he fires one and its
work stays on the department's record; he says "move this to accounting" and the agent goes.
ADR 0023 built departments as lines with toolkits in the policy file, which has no folder, no
servers of its own, no shared memory and no move. The shape below replaces it.

## Decisions

**A department is a folder, and Ghost is one too.** Everything the operator edits lives under
`~/.config/ghost/`, so the desktop offers "Open folder" instead of editors:

```
~/.config/ghost/                    # Ghost: the department every wake lands in
├── AGENTS.md                       # Ghost's standing instructions
├── mcp.json                        # Ghost's servers
├── department.json                 # Ghost's write declarations and defaults
├── memory.md                       # what every agent in Ghost knows
├── skills/<name>/SKILL.md
├── agents/ghost/                   # agent zero
├── env, settings.env, policy.toml
└── departments/
    ├── marketing/
    │   ├── AGENTS.md               # the department's rules, in every agent's prompt
    │   ├── mcp.json
    │   ├── department.json         # title, aliases, voice, write declarations
    │   ├── memory.md               # the department's shared memory, gate-written
    │   ├── skills/<name>/SKILL.md
    │   ├── docs/…                  # read on demand
    │   └── agents/<slug>/          # agent.json, persona.md, memory.md, notes.md
    └── heka/
        ├── department.json         # { "root": "~/projects/active/heka", … }
        └── agents/…
```

A department may keep its resources elsewhere: `root` in its `department.json` points at that
folder, whose `AGENTS.md`, `mcp.json` or `.mcp.json`, `skills/`, `.agents/skills/` or
`.claude/skills/`, and `docs/` are read in place. Its agents stay under
`~/.config/ghost/departments/<name>/`, so memory never lands in a repository and a move is a
rename on one disk. Ghost never writes into a `root`. Only the operator writes a `root`, by
hand or in the desktop: a server file in that folder runs commands.

**Servers are `mcp.json`, in the portable shape, with references to Ghost's.** No MCP
specification defines a client's configuration file (SEP-2633 proposes one, as a draft); the
shape most clients share is `{"mcpServers": {"<name>": {"command", "args", "env"}}}` for stdio
and `{"type": "http", "url", "headers"}` for streamable HTTP. Ghost reads exactly that, so a
server copies to and from Cursor or Claude unchanged, and follows SEP-2633 if it is accepted.
Secrets are `${NAME}` or `${env:NAME}`, resolved from Ghost's environment only; a literal
secret in the file is the operator's mistake to fix, and `ghostd check` warns on values that
look like tokens.

A department's `mcp.json` may also name one of Ghost's servers instead of starting its own:
`{"mcpServers": {…}, "use": ["github"]}`. One process serves both, it is already up when the
department is entered, and the department still reaches only what its file lists. Its levels
and write declarations come from the department's own `department.json`, never from Ghost's.

**What the gate needs lives beside it, in `department.json`.** The spec leaves permission to the
client: tool annotations are untrusted unless the server is, and clients should confirm
sensitive operations. Ghost's declarations from ADR 0022 move here unchanged in meaning, keyed by
server name: `scope`, `reads`, `writes.<operation>` (`tool`, `with`, `args`, `target`, `read`,
`asked_by`, `risk`, `bound`, `key_in`, `description`), `levels` and `targets`. A server the
operator lists is trusted, so its read-only tools are offered as they are; a write is offered
only once declared; every write goes through the gate as before. `model` is reserved here for
a department that should run a cheaper or faster model; it is not read yet. The model never
writes `mcp.json` or `department.json`.

**No toolkits.** A department reaches exactly the servers its `mcp.json` lists or uses, at the
levels its `department.json` sets, falling back to `[defaults]` in `policy.toml` and then the
operation's risk. The policy file keeps `[defaults]` and the levels of Ghost's own operations
(agents, skills); its `[mcp.*]` and `[toolkit.*]` sections go. The gate and the audit name a
department's own server `<department>/<server>` (`marketing/figma`); a used server and Ghost's
keep their bare names.

**An agent is a conversation with a persona and a job.** It is the thing the operator hires,
talks to, moves and fires; the apps, the prompts and the tool descriptions say *agent*. In the
code, the store and the control API the same object keeps the name *conversation*
(`ghost_core.conversations`, `/conversations`), since renaming them buys nothing the operator
sees. An agent is a folder in its department: `agent.json` holds a stable id, the name,
aliases, the job in a line, and a voice only when it has its own; `persona.md`, `memory.md`
and `notes.md` are Ghost's, written through the gate (ADR 0014). SQLite keys transcripts,
compactions and audit rows by the id, so a folder moved by hand keeps its history. Names are
unique across all departments. An agent without its own voice uses its department's, so a move
can be heard.

- *Hire:* `conversations.create` takes the department, the name, the job and a persona. By
  voice the persona is a short draft Ghost writes from the conversation (ADR 0019) and the
  operator edits; it is never empty. Two agents in one department on two jobs is the normal
  case.
- *Fire:* `conversations.retire` takes the agent out of the roster, voice routing and the
  prompt. Its folder, transcript and audit rows stay, and the department's journal still
  shows its work. It can be rehired. Deletion is the operator's, in the desktop.
- *Default agent:* each department has one, named for the department, with no persona of its
  own: it speaks as `AGENTS.md`. "Transfer me to marketing" lands on it. It cannot be moved,
  retired or deleted while the department exists. Ghost (agent zero) is the default agent of
  the Ghost department and is unscoped.

Per-agent scope is gone; the department's declaration is the only scope.

**Memory has two layers, and a department keeps a journal.** An agent's `memory.md` is private
to it (ADR 0014). The department's `memory.md` is shared: every agent in the department reads
it in its prompt, so a new hire knows what the department knows. Ghost writes it through the
gate only, keeps it short (a cap the prompt budget sets), and the prompt marks it as data, not
instructions. Agents in other departments do not see it, and Ghost's own is Ghost's. The
journal is a read, not a file: what each agent in a department did and when, built from the
audit tables, offered to the model as a read-only tool and shown in the desktop. A new hire
reads it to catch up; the operator reads it to see what happened while he was away.

**Ghost knows the departments and works through them.** Ghost's prompt lists each department
by name with the first line of its `AGENTS.md` and its agents by name and job. Ghost can read
a department's skills, docs and journal and transfer to it or to one of its agents; it does not
call a department's own servers, which would mean running all of them. Routing a request to the
department whose scope covers the item stays (ADR 0019). Whether an agent is working, done or
waiting on the operator is the subject of #11 and joins this listing when it exists.

**Skills, nearest first, and a skill is checked before it is offered.** A session in a
department reads its own skills, then `~/.agents/skills/`, then Ghost's
`~/.config/ghost/skills/`; the first folder with a name wins. Ghost reads its own and
`~/.agents/skills/`. `GHOST_SKILL_ROOTS` retires: a project's skills come in through a
department's `root`. A skill folder that is itself a symlink is followed, since linking skills
between tools is common; links inside a skill still are not. A folder without `SKILL.md` is
not a skill; a `SKILL.md` whose `name` differs from its folder, or that will not parse, is a
problem `ghostd check` and the desktop's Skills view name, and a skill with a problem is not
offered to the model. Skill and doc text from a `root` is untrusted like issue text.

**Servers start when a department is entered.** Ghost's connect at daemon start, and a server
a department uses is therefore already up. A department's own start as soon as a transfer into
it is decided, so the announcement and tone cover the wait, and stay up for the session plus an
idle timeout. An edit to `mcp.json` applies the next time they start. The start is on the
latency line (ADR 0018).

**"Move this agent to accounting" is one operation, in two forms.** `conversations.move`
takes the agent (the one the session is in by default) and the destination, at `confirm`; the
operator's plain request is the yes (ADR 0017).

- *Whole:* an agent moves. Its folder is renamed into the destination, the session is re-homed
  in place (new prompt, the destination's servers, skills and shared memory, its voice), and
  Ghost says "Moved to Accounting."
- *Split:* from Ghost or a department's default agent, which are long-lived and about many
  things, Ghost hires a new agent in the destination and says the name, carries this
  session's turns into it, seeds its private memory with a summary of the session it writes
  (Ghost's memory stays private, ADR 0014), and transfers there.

The proposal says which form it is. A pending proposal is cancelled. Moves are allowed in
every direction and no department can refuse one: the operator's yes is the check, and every
later write is gated at the destination's levels. A transcript can carry another department's
data and untrusted text with it; that is accepted. A persona that names the old department is
kept, and Ghost says so; it does not rewrite it. There is no undo; moving back is another move
(ADR 0015). The audit row records both departments.

**The operator edits the layer by voice, verbatim.** "Add a rule to marketing: always use UK
dates" is `departments.add_rule`, at `confirm`: it appends the operator's words, unchanged, to
the department's `AGENTS.md`, like a persona change (ADR 0014). Ghost never paraphrases,
reorders or rewrites that file; removing or changing a rule is the operator's, in the folder.
`mcp.json` and `department.json` have no voice path at all.

**Templates retire; export is the layer without the agents.** A department folder is its own
template: copy it. Export, in the desktop or by hand, is the folder minus `agents/`, so a
template carries the rules, servers, declarations, skills and docs and never a persona, a
memory or a note. Import is a copy into `departments/`. The shipped templates under
`infra/templates/` become folders in this shape. `ghost-scripts` stays, declared in a
department's `mcp.json` and scoped by skill.

**The words.** *Agent* is what the operator hires, talks to, moves and fires. *Department* is
the layer it works in. *Conversation* is the storage name for an agent and stays in code and
the control API. *Line*, *toolkit* and *template* go from the apps, the docs, the prompts, the
tool descriptions and the glossary; *thread* is accepted by voice for an agent.

## Consequences

- Core: `ghost_core.departments` (discovery, `department.json`, `root`, `memory.md`, the
  journal read), `mcp.json` parsing with `use`, per-department `Servers` started on transfer;
  `ghost_core.toolkits` and `ghost_core.templates` go; `Profile` loses `scope` and `toolkits`
  and gains a stable id, a job and a retired flag; the store keys by id; `switch()` gains a
  re-home for the same agent; `conversations.move`, `conversations.retire`,
  `departments.add_rule`.
- Skills: the three-place lookup, `.claude/skills/` in a root, symlinked folders, the
  validation, a confined read for a department's `AGENTS.md`, `docs/` and `memory.md`.
- Desktop: "Open folder" per department and agent; the Toolkits designer and template import
  go; Servers and Skills become views with problems listed; Lines becomes Agents grouped by
  department, with hire, retire and drag-to-move; export is the folder minus `agents/`.
- CLAUDE.md: the rules on toolkits and the policy file as the ceiling are rewritten; docs
  `departments.md`, `skills.md`, `mcp.md` and `GLOSSARY.md` follow once the code does.
- No migration. The lines, their files and transcripts, the toolkits, the `[mcp.*]` sections
  and the database are deleted; Ghost starts with an empty `~/.config/ghost/` layout and empty
  tables, and the operator writes Ghost's `mcp.json` and his departments fresh. The old audit
  rows held eleven writes against the mock GitHub and one test comment GitHub keeps anyway.
- Backlog, with their own issues: routines (#10) and agent status (#11).
- Order of work: the folder layout and Ghost's `mcp.json`; departments as folders with
  servers started on transfer and `use`, measured; skills lookup and validation; agents with
  hire, retire, shared memory and the journal; the move; `add_rule`; the words; the desktop.

## Considered options

- **Departments as SQLite rows with a JSON column.** Rejected: the folder is the point. A row
  needs an editor in the desktop, cannot be copied as a template or kept in a repository
  beside the department's skills and docs, and puts operator policy in the store Ghost writes.
  OpenBot stores its agents this way and had to build three export formats, each stripping
  secrets, to get a folder back. SQLite keeps only what Ghost writes: transcripts,
  compactions, the audit. The daemon reads the folders at start and when a department is
  entered.
- **Servers global to the machine, every department reaching all of them** (OpenBot's model).
  Rejected: a department reaching only its own servers is the permission model, and a
  transfer that starts a server is a cost the `use` reference removes for the shared ones
  without giving everything up.
- **A new agent with no persona, speaking as its department.** Rejected 2026-10-25: two
  agents in one department on different jobs need their own; the department's default agent
  is the one that speaks as `AGENTS.md`.
- **Memory private per agent only.** Rejected 2026-10-25: a new hire must know what the
  department knows, and the operator's "what happened in marketing" needs one place to look.
- **Channels, agents messaging each other, and routines in this ADR.** Not now. Channels and
  agent-to-agent messaging are not asked for; routines have #10.
