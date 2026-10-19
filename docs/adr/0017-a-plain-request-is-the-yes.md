# 0017: A plain request is the yes for a confirm-level change

- **Status:** accepted
- **Date:** 2026-10-19
- **Amends:** ADR 0007 (what counts as confirmation evidence), ADR 0011 (what `confirm` means)

The operator said "close infra 17" and still had to answer "Okay to close infra 17?" with a
yes. When the operator's own words already ask for exactly the change Ghost proposes, the
question is ceremony.

At `confirm`, the gate now reads the operator's words that led to the proposal: the
pipeline's transcript since Ghost last spoke, never the model's text. If they plainly ask for
that operation on that target, the gate executes at once and records the evidence as
`classifier: request` (`ghost_core.request`). Plainly means three things:

- a word the ability declares for the operation (`Operation.asked_by`, "close" or "shut" for close);
- the target named (`Target.mentions`, "17" or "seventeen"), or "it", "this" or "that one" right after
  Ghost's last reply named it;
- no negation or hedge ("no", "don't", "wait", "should", "maybe", "if", "I think") and not a question.

Otherwise Ghost asks once, as before, and any natural yes answers it: the regex allowlist that
Jev falls back to when unsure now takes "okay", "sure", "alright", "sounds good" and the like
on their own. The negation guard still runs first, so "okay, no, wait" is a no.

## Consequences

- The model proposing a change is still never evidence. Only the operator's transcribed words are,
  and the check is deterministic code, not a classifier's judgement.
- An operation that declares no `asked_by` words always asks. Abilities opt in operation by operation.
- A change Ghost inferred from something vaguer ("we're not gonna do it") still asks.
- `deny`, `announce` and `quiet` are unchanged; the policy file still sets the level.
