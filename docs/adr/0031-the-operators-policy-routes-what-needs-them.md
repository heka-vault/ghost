# 0031: The operator's policy decides how Ghost reaches them

- **Status:** proposed
- **Date:** 2026-10-27

Once something is on the waiting list (ADR 0029), Ghost has to decide how loud to be, and
the operator already suspects it could call too often. Like autonomy levels (ADR 0011),
that is the operator's call, in their file, never the model's.

## Decisions

**Routes, quietest first:** the digest (held until the next wake), desk voice (ADR 0032),
a desktop notification, the phone app's waiting list, an app call, and carrier routes
(SMS, WhatsApp, a call through a provider such as Bird). The digest and the phone app are
on; every other route is off until the operator enables it.

**A `[routes]` section in `policy.toml`** holds which routes are enabled, the urgency per
watch (`digest`, `notify`, `call`), escalation after N minutes up to a ceiling per watch,
quiet hours, a batching window, a cap on calls per hour and per day, and what an outside
route may carry. Do-not-disturb ("Ghost, nothing until two") is a fixed intent the operator
says; the model cannot set it.

**A route either gets an acknowledgment or hands the item on.** A delivery without an
acknowledgment escalates to the next enabled route when its time is up, or at once when the
route knows nobody heard (desk voice with no reply). A route that can never be
acknowledged is the last on the ladder.

**What leaves the machine is the least that works.** The phone app reads the list over the
operator's tunnel. A third-party route carries, by default, the source and a count ("CI: 1
failure"), not titles or text. There is no phone push for now (2026-10-27); when there is,
it carries no content.

**Sending through a carrier is a write** to an outside service: a declared operation
through the gate, with a level and an audit row. The daemon starts it, because the route
policy asks for it; the model does not.

## Consequences

- Every delivery and acknowledgment is recorded with the item, so "why did it call me?"
  has an answer.
- Carrier routes need the gate to run a declared operation the daemon starts, at the
  department's levels; today only Ghost's own operations run that way, at `quiet`.
