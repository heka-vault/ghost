# Skills: instructions Ghost reads on demand

A skill is a folder with a `SKILL.md`, the agent-skills standard: YAML front matter with
`name` and `description`, then the instructions, with any files beside it (ADR 0023, ADR
0024). A skill tells the model how to do a kind of task; it never changes what the model
may do.

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

Nearest first, and the first folder with a name wins:

1. **The department's own:** `skills/`, `.agents/skills/` or `.claude/skills/` under the
   department's folder, or under its `root` when it has one (a repository's own skills,
   read in place, never copied). See [departments.md](departments.md).
2. **The operator's:** `~/.agents/skills/<name>/` (`home_skills_dir`), the folder other
   agents on the machine share. Read in place.
3. **Ghost's own:** `~/.config/ghost/skills/<name>/` (`skills_dir`). The desktop adds here
   by drag-and-drop or by path, and removes; Ghost does the same by voice ("add the
   invoicing skill from my downloads") as the `skills.add` and `skills.remove` operations
   through the gate, `confirm` unless the policy file says otherwise. A skill installed
   under a new name has its `name` rewritten to match the folder.

A session in Ghost sees the operator's and Ghost's own; a session in a department sees the
department's first, then those.

## Problems

A folder without `SKILL.md` is not a skill and is ignored. A `SKILL.md` whose `name` is not
the folder's name, or whose front matter will not parse, is a **problem**: `ghostd check`,
`GET /skills` and the desktop's Skills screen list it with the reason, and it is never
offered to the model. A skill folder that is itself a symlink is followed (linking skills
between tools is common); links inside a skill are not.

## What a session gets

- An index in the system prompt: each skill's name and one line, so the model knows what
  it can read. Capped at 30 skills and 140 characters each.
- `read_skill(name, file)`: the body (6,000 characters at most) and the files beside it,
  one at a time, text only, inside the skill's folder only.

## What a skill cannot do

Skill text is untrusted like issue text. It cannot name a server the session does not have
and cannot change a level or a scope. A skill's scripts never run from its text: they run
only through `ghost-scripts`, a gated server scoped by skill ([departments.md](departments.md)).

`ghostd check` lists the skills found, where each came from, any problem, and any required
server that is not connected; `GET /skills` on the control API and the desktop's Skills
screen show the same.
