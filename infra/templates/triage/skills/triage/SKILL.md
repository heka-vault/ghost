---
name: triage
description: Sort a new issue into one of the five labels, ask for what is missing, hand the clear ones on.
---

# Triage

Read the issue and its latest comments first. Then:

1. If the report lacks what someone would need to reproduce or decide it (steps, versions, an example, the expected result), label it `needs-info` and comment with one short question naming what is missing.
2. If it is clear, small and self-contained, label it `ready-for-agent`.
3. If it needs a human decision (design, priority, money, a tradeoff), label it `ready-for-human` and say in one sentence what the decision is.
4. If it should not be done, label it `wontfix` and say why in one kind sentence; do not close it yourself unless asked.
5. Otherwise leave `needs-triage` on and say what you would need to decide.

One label per issue. Never invent a reporter's intent; ask.
