# 0030: A change at `confirm` waits on the waiting list, not in the session

- **Status:** accepted (not built yet)
- **Date:** 2026-10-27
- **Amends:** ADR 0011 (how long `confirm` waits), ADR 0017 (when the yes may come)

A change at `confirm` waits 60 seconds for the next turn of the session that proposed it
and is cancelled by a hang-up, by any other turn, or by a transfer. That fits a change the
operator just asked about. It does not fit a change a routine proposes while nobody is
talking (issue #10), or one the operator wants to think about and answer from the desk
later. The operator accepted the change on 2026-10-27 (issue #15).

## Decisions

**A pending change is an item on the waiting list** (ADR 0029), keyed by its proposal id,
not by the session. It expires after a time the policy file sets (`[defaults]
pending_ttl`, one hour unless set), not after the next turn.

**There can be more than one.** Confirm and cancel take a proposal id. The control API's
`/proposals/pending/confirm` without an id keeps meaning "the one this session is waiting
on", so the desktop's current Confirm button works unchanged.

**What still holds:**

- The evidence is the operator's: a spoken yes in the pipeline's own transcript, the
  operator's plain request (ADR 0017), or a Confirm in the operator's desktop (ADR 0013).
  The model is never evidence.
- In the session that proposed it, the next turn answers it as before: a clear yes applies
  it, a no cancels it. A turn about something else no longer cancels it; it stays on the
  list.
- A hang-up or a transfer leaves it on the list instead of cancelling it.
- Confirming later runs the stale check against the stored snapshot first; a target that
  changed since is refused and the item says why.
- A later spoken yes must name it ("yes to the CI issue") or follow Ghost reading it out;
  a bare "yes" with nothing pending in the conversation answers nothing.

## Consequences

- The gate keeps proposals by id and stops cancelling them on a mismatched turn; the
  voice layer's hang-up and transfer stop cancelling.
- `useGhost` and the phone's `Desk` hold a list of pending changes.
- The audit row says when and from where the yes came, as today.
