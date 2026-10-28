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
requires: [app:quickbooks]          # optional: apps it needs; the desktop shows "missing"
---
```

## Where skills come from

In this order, and the first folder with a name wins:

1. **Ghost's own:** `~/.config/ghost/skills/<name>/` (`skills_dir`). The desktop adds here
   by drag-and-drop or by path, and removes; Ghost does the same by voice ("add the
   invoicing skill from my downloads") as the `skills.add` and `skills.remove` operations
   through the gate, which run and Ghost says so unless the permissions ask first. A skill
   installed under a new name has its `name` rewritten to match the folder.
2. **The operator's:** `~/.agents/skills/<name>/` (`home_skills_dir`), the folder other
   agents on the machine share. Read in place.

## Problems

A folder without `SKILL.md` is not a skill and is ignored. A `SKILL.md` whose `name` is not
the folder's name, or whose front matter will not parse, is a **problem**: `ghostd check`,
`GET /skills` and the desktop's Skills page (Settings › Skills) list it with the reason, and it is never
offered to the model. A skill folder that is itself a symlink is followed (linking skills
between tools is common); links inside a skill are not.

## What a session gets

- An index in the system prompt: each skill's name and one line, so the model knows what
  it can read. Capped at 30 skills and 140 characters each.
- `read_skill(name, file)`: the body (6,000 characters at most) and the files beside it,
  one at a time, text only, inside the skill's folder only.

## What a skill cannot do

Skill text is untrusted like issue text. It cannot name an app the session does not have
and cannot change a permission. A skill's scripts never run from its text: they run only
through `ghost-scripts`, an app whose `run` goes through the gate like any change
([folder.md](folder.md#scripts-run-only-through-the-gate)).

`ghostd check` lists the skills found, where each came from, any problem, and any app it
requires that is not connected (`mcp:<name>`, the older form, still reads); `GET /skills` on the control API and the desktop's Skills
page show the same.
