# 0018: Performance is a feature, measured every release

- **Status:** accepted
- **Date:** 2026-10-19

Ghost is spoken to. Delay is the first thing the operator notices, and in testing the
phone over the LAN there were turns where Ghost took ten seconds or more to answer. Ghost is
pre-1.0 and most work is still making the proof of concept work. But speed from the desk,
the phone and the desktop app is a product requirement, not a later polish pass.

[docs/performance.md](../performance.md) is the charter. It sets out the stages of a turn
we time, the budget for each, and how we work on it. Every live turn records a
timings-only timeline (`latency.jsonl`, `make latency`). A headless harness (`make perf`)
drives scripted conversations through the real pipeline and reports p50 and p95 per stage.
Every release runs it and keeps the report.

## Consequences

- A change made to speed things up shows before and after numbers. A change that adds
  a step to a turn says what that step costs.
- Choices like the model, the thinking mode and caching are settled with the harness, not
  by preference.
- Delay added on purpose (the turn grace, holding a short reply until it's complete) is a
  setting with a measured cost. It isn't hidden.
- Speed never overrides the privacy, gate or secrets rules.
