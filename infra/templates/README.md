# Shipped departments

Each folder here is a department in the shape ADR 0024 fixes: `AGENTS.md` (its rules, in
every agent's prompt), `mcp.json` (its servers, or `use` for Ghost's), `department.json`
(what the gate needs: reads, scope, writes and levels per server) and any `skills/`. To use
one, copy the folder into `~/.config/ghost/departments/` and edit it; the desktop's
Departments screen shows it on the next refresh and gives it its own agent. Nothing here
runs until you copy it.

- `triage`: labels GitHub issues through Ghost's own `github` server (`use`), with the five
  triage labels. Edit the `items` to your repositories.
- `accounting`: a placeholder `books` server to replace with your bookkeeping server's
  command, plus the `month-close` skill.
- `design-research`: read-only Mobbin searches through the fleet's MCP proxy.
