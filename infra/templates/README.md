# Shipped departments

Each folder here is a department in the shape ADR 0024 fixes: `AGENTS.md` (its rules, in
every agent's prompt), `department.json` (its title, aliases and voice, and the permissions
that narrow what its agents may do) and any `skills/`. To use one, copy the folder into
`~/.config/ghost/departments/` and edit it; the desktop's Departments screen shows it on the
next refresh and gives it its own agent. Nothing here runs until you copy it.

Apps are connected to Ghost once, in the desktop's Apps screen, and every department reaches
them (ADR 0034). A department can only add care: `block`, `ask` and `quiet` rules over the
operator's own.

- `triage`: works GitHub issues with the five triage labels. Its permissions keep it to the
  `ghost-mock` repositories and ask before closing one; edit the owner to yours.
- `accounting`: asks before every change to a bookkeeping app called `books` (connect yours
  under that name), plus the `month-close` skill.
- `design-research`: Mobbin searches; connect Mobbin in the Apps screen.
