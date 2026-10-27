# 0033: Ghost makes things, into an outputs folder, through the gate; it is not an IDE

- **Status:** proposed
- **Date:** 2026-10-27

Asked for a presentation, Ghost should write the HTML and JavaScript page itself, not hand
it to a coding tool ([direction.md](../direction.md)). Nothing in the gate can carry that:
department memory and rules take a few hundred characters, and `docs/` is read-only.
What Ghost is not is a software development harness: sustained engineering inside a
codebase is a coding tool's work.

## Decisions

**`outputs.write`** creates or replaces one file in the active department's `out/` folder
(Ghost's is `~/.config/ghost/out/`). It is a gate operation like any other: level, audit,
read-back.

- The name is a plain file name, no directories, no links, no dot files.
- The extension is on an allowlist: `.html`, `.md`, `.txt`, `.csv`, `.json`, `.svg`, `.py`,
  `.sh`.
- The content is capped (256 KB).
- Creating a file is low risk (`announce`); replacing one is medium (`confirm`), unless the
  policy file says otherwise.

**Ghost never runs what it makes.** A page is opened by the operator; a script runs only
if the operator puts it in a skill, through `ghost-scripts` (ADR 0023).

**Ghost says where it is**, in a few words ("It's in accounting's outputs."), and the
desktop lists a department's outputs with Reveal and Open.

**The line with coding tools** is in the prompt: a self-contained thing is Ghost's to make;
work inside a project's codebase is a coding tool's, which Ghost can observe (ADR 0028).
