# 0028: Watches observe outside systems, deterministically first

- **Status:** accepted (built 2026-10-27 for polled watches: `ghost_core.watches`, `Fleet.watch_read`; pushed ones are ADR 0027's listener)
- **Date:** 2026-10-27

Ghost is to notice things on its own ([direction.md](../direction.md)): a red build, a
message from a named sender, a job that failed. Most of those questions have deterministic
answers, and a model on a timer (OpenClaw's heartbeat) spends tokens to learn that nothing
happened. Ghost already listens that way: local models until the wake phrase fires (ADR
0010). It observes the same way.

## Decisions

**A watch is declared by the operator, in a department's `department.json`**, under
`watches`, and never written by the model. It is one of two kinds:

- **Polled:** a read the department already declares (ADR 0022), its arguments and an
  interval. The daemon calls the read itself; no model is involved.
- **Pushed:** a signed request to the daemon, from the workstation, the LAN or the tunnel,
  or from the internet on ADR 0027's terms.

**It keeps typed fields only.** A watch names the fields it takes from each result (a path
and a type), and everything else is dropped. An observation is those fields.

**It decides in declared steps, cheapest first:**

1. A deterministic check over the fields (a path, `in`, `exists`, `==`, `>`, and an
   optional "for at least N minutes"). Most watches stop here.
2. Jev, when the watch declares a typed question; its answer must be one of the watch's
   declared values, and on a failure, a timeout or low confidence the declared default
   applies.
3. A model, when the watch declares one, choosing among the same values or writing the
   item's summary.

Each step sees only the fields that passed the step before. None can pick a value the
watch didn't declare, raise urgency past the watch's ceiling, or grant anything. A watch
names which fields may leave the machine for steps 2 and 3.

**What it produces is an item with a stable key**, built from a template over the fields,
so the same failure is one item however often it is seen. Ghost diffs what it sees: a new
key is news, a key that disappears is resolved.

**Everything a watch reads is untrusted data**, like issue text and MCP results (ADR 0022):
it never changes a level, a scope, a prompt, a rule or a response.

## Consequences

- A department server with a watch stays running while the daemon runs, instead of
  starting on the first transfer and stopping when idle (ADR 0024), and a dead transport
  is reconnected with backoff.
- A watch's read is not trimmed to a spoken turn's size; it has its own cap.
- The cost of an idle daemon with watches is measured (ADR 0018).
