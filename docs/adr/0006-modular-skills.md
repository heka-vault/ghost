# 0006: Ghost's capabilities are modular skills

- **Status:** accepted; superseded in form by ADR 0012 (skills are now abilities, installable packages discovered by entry point)
- **Date:** 2026-10-16

GitHub issue triage is Ghost's first skill, not its whole identity. A skill is a package that declares its read tools for the LLM, the typed write operations it can `prepare`, its config, and its prompt fragment. The agent loads enabled skills from config; it has no skill-specific code. Writes from any skill go through the same domain gate (ADR 0007).

## Consequences

- Issues on a self-hosted Forgejo can be a later skill or a second provider for the issue skill, without touching the agent.
- The skill interface is the main seam to design carefully in the first increment.
