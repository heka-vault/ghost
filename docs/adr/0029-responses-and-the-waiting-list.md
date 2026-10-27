# 0029: A watch's response records, tells or acts; what needs the operator is one list

- **Status:** proposed
- **Date:** 2026-10-27

An observation (ADR 0028) has to lead somewhere, and "Ghost, what needs me?" should be
answered from a list Ghost keeps, not by a model searching every system.

## Decisions

**A response is the operator's, per watch:** `record` (kept for the digest and for "what
happened overnight?"), `tell` (the item goes on the waiting list), or `act` (a routine
runs in the department with the item's fields as data, issue #10). A watch may declare
`tell` and `act` together. The model never picks a response. *Rule* keeps its meaning, a
line of a department's `AGENTS.md`.

**The waiting list** is what needs the operator: the items from `tell`, and Ghost's own:
a change at `confirm` with no yes (ADR 0030), a server that failed, a department with a
problem. It is a projection over the store, not a second source of truth.

**An item** has a stable key, a source, a department, a title, a link, when it was first
and last seen, and when it was resolved, delivered, acknowledged or snoozed. It leaves the
list when it is resolved at its source.

**Delivered is not acknowledged.** Ghost having said something, or a notification having
been sent, is a delivery. Only the operator acknowledging it (a spoken reply, opening it,
pressing acknowledge) stops it alerting; a snooze stops it until a time. An item that
changes alerts again.

**"What needs me?" reads the list.** The model gets a read tool that returns the list's
typed fields: key, source, department, title, age, state. Nothing else.

## Consequences

- The control API gains `/waiting` with acknowledge and snooze; `Status` carries the count;
  the bus carries a `waiting` event; the phone listener serves the same list.
- Acknowledging and snoozing are not gate writes: they change Ghost's own bookkeeping, not
  an outside system, so a button may do them.
