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
   agents on the machine share. Read in place, never written: a skill installed with
   `npx skills` shows up here, so the CLI and Ghost share one list.

## Finding and installing skills (#35)

Settings › Skills has **Find skills**: a search of [skills.sh](https://skills.sh), the open
directory behind the `npx skills` CLI, and **Install from GitHub…** for a skill it doesn't
list. Everything here is the operator's doing, from the desktop (`ghost_core::skill_catalog`,
`ghost_api::skill_catalog`).

- **Search.** skills.sh's documented API needs a Vercel token Ghost can't have, so Ghost uses
  the site's public search (`GET https://skills.sh/api/search?q=`), which is undocumented.
  The daemon asks only when the page does, for two letters or more, and keeps each query's
  results for an hour. Its answer is checked field by field; a result that isn't a skill in
  a GitHub repository (skills.sh also lists skills websites serve) is left out. When the
  site fails or answers in a shape Ghost doesn't know, the page says so, shows the last
  results it had for the search, and installing from a GitHub address still works.
- **Look before installing.** A result, or an address (`owner/repo/skill`, `owner/repo`
  with one skill, or a `github.com` link to a skill's folder or its SKILL.md), is fetched
  from GitHub: the branch (the default one unless the link names another) resolved to a
  commit, the commit's tree read, the folder picked (by name, else by the name its SKILL.md
  gives itself), and every file downloaded at that commit and checked against the tree's
  blob hash. The folder is staged under `~/.cache/ghost/skill-previews/` (cleared at start;
  a look lasts 30 minutes) and the operator reads the whole SKILL.md and the list of
  scripts before anything is installed. All of it is the source's text. What is installed
  is that staged folder, so nothing goes in that the operator wasn't shown; the only change
  is the `name` in its front matter, set to the folder it is installed as.
- **Which skill.** A repository with more than one skill asks which, naming each by its
  folder's path (`skills/here-now`, `.claude/skills/here-now`). When two skills share a
  name, Ghost never picks one: it asks for the folder's `github.com` address.
- **Limits.** A link or a submodule in the folder refuses the skill, as does a path that
  would leave it, more than 200 files, more than 2 MB, or a SKILL.md over 60 KB, longer
  than the look shows whole (checked from the tree before anything is downloaded, and the
  SKILL.md again once it is). A long SKILL.md is refused, never cut. Files are written 0644: nothing is made runnable, and nothing is
  run. A script still runs only through `ghost-scripts`, as a change through the gate.
- **Install** is `skills.install`, a sibling of `skills.add` that the gate registers and the
  model is never offered: it copies the staged folder into `~/.config/ghost/skills/<name>/`
  through the gate (audited, and the operator's permissions can block it, by repository
  too: its argument is `source`). Where it came from goes in
  `~/.config/ghost/skills/.skill-lock.json`, in the shape of the CLI's
  `~/.agents/.skill-lock.json` (`source`, `sourceUrl`, `skillPath`, `skillFolderHash`) plus
  the `commit`. The folder hash is git's tree hash of the skill's folder, as the CLI's is.
  A skill of the same name in `~/.agents/skills` is never hidden: install it under another
  name. Adding a local folder over an installed skill, or removing it, drops its entry.
- **Updates.** Each installed skill shows its source and commit. When the page opens it asks
  Ghost once whether each source has moved on: the branch's commit now, and if that changed,
  whether the skill's folder hash did (a change elsewhere in the repository is not an
  update). Ghost asks GitHub then, never in the background and never on a turn. **Update…**
  fetches the new version for the same look, and installs it over the old one: edits the
  operator made to Ghost's copy are replaced, and the look says so. The new copy is made
  beside the old and swapped in by renaming, so an update that fails partway leaves the
  old skill and its `.skill-lock.json` entry as they were.
- **The network.** Ghost talks to skills.sh, `api.github.com` (two requests a look, one or
  two a skill per update check) and `raw.githubusercontent.com`, without signing in, so
  GitHub's limit of 60 API requests an hour applies; when it is used up, the page says until
  when. The catalog's search results, the look before installing and the update check are
  the operator's only: no session queries the catalog or GitHub, and none of it reaches the
  model. An installed skill does: that is what it is for. Like any skill, the model reads
  its SKILL.md and files on demand with `read_skill`, as untrusted text (below).

Control API: `GET /skill-catalog?search=`, `POST /skill-catalog/preview`,
`POST /skill-catalog/install`, `GET /skill-catalog/updates` and
`POST /skill-catalog/updates/{name}`; `GET /skills` carries each skill's `origin`.

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
