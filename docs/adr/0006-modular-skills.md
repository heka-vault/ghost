# 0006: Ghost's capabilities are modular skills

- **Status:** superseded by ADR 0012, then by ADR 0022 (capabilities are MCP servers the operator declares)
- **Date:** 2026-10-16

GitHub issue triage is Ghost's first skill, not its whole identity. A skill is a package that declares its read tools for the LLM, the typed write operations it can `prepare`, its config, and its prompt fragment. The agent loads enabled skills from config; it has no skill-specific code. Writes from any skill go through the same domain gate (ADR 0007).

## Consequences

- Issues on a self-hosted Forgejo can be a later skill or a second provider for the issue skill, without touching the agent.
- The skill interface is the main seam to design carefully in the first increment.
