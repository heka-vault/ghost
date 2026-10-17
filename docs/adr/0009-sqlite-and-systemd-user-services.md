# 0009: SQLite for state; systemd user services started directly with uv

- **Status:** accepted
- **Date:** 2026-10-16
- **Supersedes:** original spec's PostgreSQL, Prisma, and Docker Compose

One user and one host: SQLite in WAL mode holds checkpoints, proposals, operations, and audit events, with unique constraints for idempotency keys. Ghost runs as `systemd --user` units launched with `uv run`, no containers. The desktop daemon needs direct PipeWire access anyway, and there is nothing to orchestrate.
