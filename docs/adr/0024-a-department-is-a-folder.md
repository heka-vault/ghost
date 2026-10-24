# 0024: A department is a folder; conversations move between them

- **Status:** proposed
- **Date:** 2026-10-22
- **Supersedes, when accepted:** ADR 0023's toolkits, templates and departments-as-lines; ADR
  0022's servers in the policy file; ADR 0019's per-conversation scope; the word *line*
  (ADR 0014, GLOSSARY.md)

The operator wants a department to be a folder he can open and edit, holding its own servers,
skills and rules, and wants to say "let's move this conversation to the accounting department".
ADR 0023 built departments as lines with toolkits in the policy file, which has no folder, no
servers of its own, and no move. The review (2026-10-21 and 22) settled the shape below.

## Decisions

**A department is a folder, and Ghost is one too.** Everything the operator edits lives under
`~/.config/ghost/`, so the desktop offers "Open folder" instead of editors:

```
~/.config/ghost/                    # Ghost: the department every wake lands in
├── AGENTS.md                       # Ghost's standing instructions
├── mcp.json                        # Ghost's servers
├── department.json                 # Ghost's write declarations and defaults
├── skills/<name>/SKILL.md
├── conversations/ghost/            # conversation zero
├── env, settings.env, policy.toml
└── departments/
    ├── accounting/
    │   ├── AGENTS.md               # the department's rules, in every conversation's prompt
    │   ├── mcp.json
    │   ├── department.json         # title, aliases, voice, write declarations
    │   ├── skills/<name>/SKILL.md
    │   ├── docs/…                  # read on demand
    │   └── conversations/<slug>/   # conversation.json, persona.md, memory.md, notes.md
    └── heka/
        ├── department.json         # { "root": "~/projects/active/heka", … }
        └── conversations/…
```

A department may keep its resources elsewhere: `root` in its `department.json` points at that
folder, whose `AGENTS.md`, `mcp.json` or `.mcp.json`, `skills/` or `.agents/skills/` and `docs/`
are read in place. Its conversations stay under `~/.config/ghost/departments/<name>/`, so memory
never lands in a repository and a move is a rename on one disk. Only the operator writes a
`root`, by hand or in the desktop: a server file in that folder runs commands.

**Servers are `mcp.json`, in the portable shape.** No MCP specification defines a client's
configuration file (SEP-2633 proposes one, as a draft); the shape most clients share is
`{"mcpServers": {"<name>": {"command", "args", "env"}}}` for stdio and
`{"type": "http", "url", "headers"}` for streamable HTTP. Ghost reads exactly that, so a server
copies to and from Cursor or Claude unchanged, and follows SEP-2633 if it is accepted. Secrets
are `${NAME}` or `${env:NAME}`, resolved from Ghost's environment only; a literal secret in the
file is the operator's mistake to fix, and `ghostd check` warns on values that look like tokens.

**What the gate needs lives beside it, in `department.json`.** The spec leaves permission to the
client: tool annotations are untrusted unless the server is, and clients should confirm
sensitive operations. Ghost's declarations from ADR 0022 move here unchanged in meaning, keyed by
server name: `scope`, `reads`, `writes.<operation>` (`tool`, `with`, `args`, `target`, `read`,
`asked_by`, `risk`, `bound`, `key_in`, `description`), `levels` and `targets`. A server the
operator lists is trusted, so its read-only tools are offered as they are; a write is offered
only once declared; every write goes through the gate as before. The model never writes
`mcp.json` or `department.json`.

**No toolkits.** A department reaches exactly the servers its `mcp.json` lists, at the levels
its `department.json` sets, falling back to `[defaults]` in `policy.toml` and then the operation's
risk. Two departments that use one service each declare it; with few servers that is cheap. The
policy file keeps `[defaults]` and the levels of Ghost's own operations (conversations, skills);
its `[mcp.*]` and `[toolkit.*]` sections go. The gate and the audit name a department's server
`<department>/<server>` (`accounting/books`); Ghost's keep their bare names.

**Ghost knows the departments and works through them.** Ghost's prompt lists each department
by name with the first line of its `AGENTS.md`. Ghost can read a department's skills and docs
and transfer to it; it does not call a department's servers, which would mean running all of
them. Routing a request to the department whose scope covers the item stays (ADR 0019).

**Skills, nearest first.** A session in a department reads its own skills, then
`~/.agents/skills/`, then Ghost's `~/.config/ghost/skills/`; the first folder with a name wins.
Ghost reads its own and `~/.agents/skills/`. `GHOST_SKILL_ROOTS` retires: a project's skills
come in through a department's `root`. A skill folder that is itself a symlink is followed,
since linking skills between tools is common; links inside a skill still are not. Skill and
doc text from a `root` is untrusted like issue text.

**Servers start when a department is entered.** Ghost's connect at daemon start. A
department's start as soon as a transfer into it is decided, so the announcement and tone
cover the wait, and stay up for the session plus an idle timeout. An edit to `mcp.json` applies
the next time they start. The start is on the latency line (ADR 0018).

**A conversation is a folder in its department.** `conversation.json` holds a stable id, the
name, aliases, and a voice only when it has its own. The folder's place is the department it is
in; SQLite keys transcripts, compactions and audit rows by the id, so a folder moved by hand
keeps its history. Names are unique across all departments. Each department has a default
conversation, named for it, where "transfer me to accounting" lands; it cannot be moved or
deleted while the department exists. A new conversation's persona is empty: it speaks as its
department's `AGENTS.md` until the operator gives it one. A conversation without its own voice
uses its department's, so a move can be heard. Per-conversation scope is gone; the
department's declaration is the only scope.

**"Move this conversation to accounting" is one operation, in two forms.**
`conversations.move` takes the conversation (the one the session is in by default) and the
destination, at `confirm`; the operator's plain request is the yes (ADR 0017).

- *Whole:* a topic conversation moves. Its folder is renamed into the destination, the session
  is re-homed in place (new prompt, the destination's servers and skills, its voice), and Ghost
  says "Moved to Accounting."
- *Split:* from Ghost or a department's default conversation, which are long-lived and about
  many things, Ghost names a new conversation in the destination and says the name, carries
  this session's turns into it, seeds its memory with a summary of the session it writes (Ghost's
  memory stays private, ADR 0014), and transfers there.

The proposal says which form it is. A pending proposal is cancelled. Moves are allowed in
every direction and no department can refuse one: the operator's yes is the check, and every
later write is gated at the destination's levels. A transcript can carry another department's
data and untrusted text with it; that is accepted. A persona that names the old department is
kept, and Ghost says so; it does not rewrite it. There is no undo; moving back is another move
(ADR 0015). The audit row records both departments.

**Templates retire.** A department folder is its own template: copy it. The shipped ones under
`infra/templates/` become folders in this shape, the desktop's import is a copy, and export is
the folder. `ghost-scripts` stays, declared in a department's `mcp.json` and scoped by skill.

**One word: conversation.** *Line* goes from the apps, the docs, the prompts and the tool
descriptions; *thread* is accepted by voice. *Toolkit* and *template* leave the glossary.

## Consequences

- Core: `ghost_core.departments` (discovery, `department.json`, `root`), `mcp.json` parsing and
  per-department `Servers` started on transfer; `ghost_core.toolkits` and
  `ghost_core.templates` go; `Profile` loses `scope` and `toolkits` and gains a stable id;
  the store keys by id; `switch()` gains a re-home for the same conversation; `conversations.move`.
- Skills: the three-place lookup, symlinked folders, a confined read for a department's
  `AGENTS.md` and `docs/`.
- Desktop: "Open folder" per department and conversation; the Toolkits designer and template
  import go; Servers and Skills become views with problems listed; Lines becomes Conversations
  grouped by department, and dragging one to another department is the move.
- CLAUDE.md: the rules on toolkits and the policy file as the ceiling are rewritten; docs
  `departments.md`, `skills.md`, `mcp.md` and `GLOSSARY.md` follow.
- No migration. The lines, their files and transcripts, the toolkits and the `[mcp.*]`
  sections are deleted; Ghost starts with an empty `~/.config/ghost/` layout and the
  operator writes Ghost's `mcp.json` and his departments fresh. The audit table stays: it is
  the record of writes already made to outside services.
- Order of work: the folder layout and Ghost's `mcp.json`; departments as folders with
  servers started on transfer, measured; skills lookup; the move; the rename to
  *conversation*; the desktop.

## Considered options

- **Departments as SQLite rows with a JSON column.** Rejected: the folder is the point. A row
  needs an editor in the desktop, cannot be copied as a template or kept in a repository
  beside the department's skills and docs, and puts operator policy in the store Ghost writes.
  SQLite keeps only what Ghost writes: transcripts, compactions, the audit. The daemon reads
  the folders at start and when a department is entered.
