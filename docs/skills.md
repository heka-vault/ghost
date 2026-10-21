# Skills: instructions Ghost reads on demand

A skill is a folder with a `SKILL.md`, the agent-skills standard: YAML front matter with
`name` and `description`, then the instructions, with any files beside it (ADR 0023). A
skill tells the model how to do a kind of task; it never changes what the model may do.

```
~/.config/ghost/skills/
└── month-close/
    ├── SKILL.md          # ---\nname: month-close\ndescription: Close the books each month.\n---
    └── checklist.md
```

```yaml
---
name: month-close
description: Close the books each month, from reconciliation to the report.
requires: [mcp:quickbooks]          # optional: servers it needs; the desktop shows "missing"
---
```

## Where skills come from

- **Ghost's own:** `~/.config/ghost/skills/<name>/` (`GHOST_SKILLS_DIR`). The desktop adds
  here by drag-and-drop or by path, and removes; Ghost does the same by voice ("add the
  invoicing skill from my downloads") as the `skills.add` and `skills.remove` operations
  through the gate, `confirm` unless the policy file says otherwise.
- **A project's:** `<root>/.agents/skills/<name>/` for each root in `GHOST_SKILL_ROOTS`
  (paths separated by commas, `~` allowed), read in place, never copied or removed. The
  fleet's own convention.

The first folder with a name wins; a later one with the same name is logged and left out.

## What a session gets

- An index in the system prompt: each skill's name and one line, so the model knows what
  it can read. Capped at 30 skills and 140 characters each.
- `read_skill(name, file)`: the body (6,000 characters at most) and the files beside it,
  one at a time, text only, inside the skill's folder only.
- With departments and toolkits (ADR 0023, later phases) a session sees only the skills its
  toolkits grant; today every session sees every skill.

## What a skill cannot do

Skill text is untrusted like issue text. It cannot name a server the session does not have,
cannot change a level or a scope, and nothing in it is run: a skill's scripts wait for the
gated scripts server (ADR 0023), and until then are just files the model can read.

`ghostd check` lists the skills found and any required server that is not connected;
`GET /skills` on the control API and the desktop's Skills screen show the same.
