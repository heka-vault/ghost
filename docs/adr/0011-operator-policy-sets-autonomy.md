# 0011: The operator's policy sets autonomy per operation; the gate bounds and records every write

- **Status:** accepted; amended by ADR 0015 (no undo); amended by ADR 0030 (a pending change waits on the waiting list)
- **Date:** 2026-10-17
- **Amends:** ADR 0007 (the confirmation clause)

ADR 0007 made every write wait for a verbatim readback and a spoken "confirm". That
turned out to bundle two different things: the safety invariants, and one particular
level of caution. Reading back "labels: add bug" and waiting for "confirm" on every
label change made Ghost feel like a form. The operator, not the architecture, should
decide how much the assistant may do on its own.

The model decides *what* to write, as typed, validated change sets. The operator's
policy decides *whether it may happen without asking*. The gate does the same work
either way.

**Autonomy levels**, per skill operation, with per-target overrides, in a TOML file
the operator owns (`~/.config/ghost/policy.toml`, template in `infra/policy.example.toml`):

| Level | Behaviour |
|---|---|
| `deny` | not offered to the model |
| `confirm` | the ADR 0007 flow: propose, ask, wait for a spoken yes, apply |
| `announce` | apply now, say in a few words what happened |
| `quiet` | apply now, mention only if asked or if it failed |

Skills suggest a default per operation by risk class (low: announce; medium and high:
confirm). Resolution order: target rule, skill rule, `[defaults]`, risk class. Ghost
never writes the file; issue and comment text never reaches it.

**Confirmation classification** is pluggable. Jev (TypeSafe System One) judges the
operator's words when a key is configured (`GHOST_JEV=auto|on|off`); the regex
allowlist is the fallback on no key, timeout, error, or low confidence. Both apply a
negation guard first. The evidence is still the pipeline's own transcript, never
model output.

**Undo** (removed by ADR 0015; Ghost reverses a change with an ordinary change). Because writes can now happen without asking, the last executed change set
in a session can be reversed within ten minutes: labels and state from the stored
snapshot, comments by their hidden marker. Undo is itself an audited write.

## What does not change

- Tool arguments are untrusted; skills validate them into typed change sets.
- The target allowlist is enforced in code (ADR 0008), before autonomy is consulted.
- Every write at every level is bounded, idempotent, stale-checked, read back, and
  recorded with the level that applied.
- The model speaks verified outcomes, never guesses.
- Permission is never a model's decision. Jev classifies speech; it does not set policy.

## Consequences

- "Label it bug" gets "Done" instead of a readback and a wait. Closing an issue still
  asks, until the operator relaxes it.
- Risk moves from the confirmation step to the policy file and the undo path. Both
  are the operator's to tune; a new skill's operations start at their risk-class
  defaults, and the operator tightens or loosens from there.
- `GHOST_DEBUG_AUDIO` logs each gate decision and each classifier verdict.
