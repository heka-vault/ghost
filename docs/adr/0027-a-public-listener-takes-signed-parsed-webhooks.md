# 0027: A public listener may take webhooks: signed, parsed by declaration, checked at the source

- **Status:** accepted (built 2026-10-27: `ghost_core.hooks`, `ghostd.hooks`, `ghostd push`); amended by ADR 0034 (the check reads through an app's read-only tool, `app` for `server`)
- **Date:** 2026-10-27
- **Amends:** ADR 0020 (a public listener is no longer ruled out; the phone path is unchanged)

Ghost is to observe outside systems and act on what they report ([direction.md](../direction.md)).
Anything on the workstation, the LAN or the operator's WireGuard network can already reach
the desk, but hosted services (GitHub, Bird, a monitoring service) send webhooks from the
internet, and ADR 0020 rejected a public listener. The operator's decision: a public
listener is allowed, and ADR 0020's real point stands as a constraint on it, so that what
faces the internet cannot be used to steer Ghost. Polling stays the fallback for anything
that cannot meet these rules.

## Decisions

**Deterministic first, the model last.** Everything between the internet and the model is
deterministic code that can be tested: verify, parse, constrain, check at the source. Only
what survives all of it is handed to a non-deterministic system, and then only as typed
fields. The rules below are that principle applied to a webhook.

**One narrow listener, for webhooks only.** It serves `POST /hooks/<source>` for the sources
the operator declares, and nothing else: no phone route, no `/pair`, nothing of the control
API (ADR 0013), no static files, no proxy. An unknown source is a 404 with no body read.

**It runs in `ghostd`**, as a third app beside the control API and the phone listener,
the way the phone listener does (ADR 0016): its own addresses (`GHOST_HOOKS_LISTEN`, off
when empty), no routes or middleware shared with the other two, and a request handled
here reaches the rest of the daemon only as a parsed observation. How the internet
reaches that address (a forwarded port, the WireGuard hub) is the operator's network and
is not in this repository.

**Authenticated before it is read.** Every source verifies a signature over the raw bytes
with a secret from the environment (`${NAME}`), in the source's own scheme (GitHub's
`X-Hub-Signature-256`, the provider's documented scheme, or Ghost's generic HMAC over
`<timestamp>.<delivery id>.<body>`), inside a clock window, with the delivery id deduped.
The body is size-capped before verification. A request that fails any check is dropped:
its body is not parsed, stored or logged, only counted.

**Parsed by declaration, deterministically, never by a model.** Each source declares, in
the department folder that owns it, which events it accepts and which fields it takes
from each: the JSON path, the type, and a constraint (a pattern, a length, an enum, or
membership in the department's allowlist). Parsing is code in `ghost_core`, one parser
for every source, reading those declarations: no per-service handler. A body that does not
match a declared event, or a field that fails its constraint, is rejected whole. What
survives is a typed observation with a stable key; the rest of the body is discarded at
the door.

**Checked at the source before it is believed.** A parsed reference (a repository and run
id, an issue URL) is re-read through the department's declared MCP read before the
observation counts: the webhook says run 123 failed, Ghost reads run 123 from GitHub. The
read's answer is the truth; the webhook is a hint about where to look. A reference outside
the department's allowlist is rejected without a read. A source with no read to check
against is allowed, and its observations are marked unchecked, which the operator's rules
can treat differently (record only, say).

**No raw text reaches the model.** The model sees an observation's typed fields, never the
body. A free-text field (a commit message, a PR title) is taken only if the declaration
names it, with a length cap, and it reaches the model the way issue text does: as quoted
data the model is told not to follow or repeat. By default no free text is taken.

**A webhook can ask for nothing.** It produces an observation and the operator's rules
decide what happens: record, tell, or act (a routine, #10). It cannot write, confirm a
change, stand in for the operator's yes, change a level, a scope, a rule or a prompt, or
add a source. A routine it starts runs at the department's levels, and at `confirm` it
waits for the operator.

**Kept as parsed.** The store keeps the parsed fields, the check's result and the
delivery id, never the raw body.

**Tested with recorded deliveries.** Because every step before the model is
deterministic, every step is tested without one. Each declared source ships fixtures
beside its declaration: real deliveries (secrets and personal data scrubbed) that must
parse to an exact observation, and bad ones (wrong signature, stale timestamp, replayed
id, a field that breaks its constraint, an item outside the allowlist, an oversized body,
instructions in a free-text field) that must be rejected or carried as inert data.
`ghostd check` runs a department's fixtures, and a declaration that fails its own
fixtures is a problem the folder reports, so the source stays off.

## Considered options

- **Keep ADR 0020's rule and poll everything.** Still the fallback, and enough for most
  sources at a minute's delay. Rejected as the only path because some sources only push
  and some events matter within seconds.
- **Hand the body to the model to interpret.** Rejected: a webhook body is attacker-shaped
  text from the internet, and a model reading it is the prompt-injection path the
  untrusted-data rule exists to close.
- **A handler per service in code.** Rejected for the same reason ADR 0022 retired
  bespoke clients; the declaration covers what a handler would.

## Consequences

- The phone path is ADR 0020 unchanged: LAN or WireGuard only, never through this
  listener.
- `department.json` grows a `hooks` declaration per source: verification scheme, secret
  name, events, fields with their constraints, the key and title templates, and the read
  that checks the reference. `ghostd check` and the desktop show each source's state and
  its rejection counts.
- Re-reading at the source costs one read per observation (ADR 0018: measured with the
  rest of the turn-free path, not on the voice path).
- A two-way phone call over a carrier (Bird's call audio into the pipeline) is media, not
  a webhook, and is not covered here; it needs its own decision.
