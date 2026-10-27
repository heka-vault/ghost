# Ghost's direction: chief of staff, not an IDE

**Status:** draft for review, 2026-10-27. Not an ADR. Where it settles something, an ADR
follows; where it disagrees with an accepted ADR, the ADR wins until it is amended.

## In one sentence

Ghost observes the systems the operator cares about, is told when they change, knows
what needs the operator's attention, and acts on it: it tells them, asks them, or does
the work itself through the gate.

## Why write this down

Every assistant drifts toward becoming a software development harness: a model, a loop,
a shell, a file editor, a sandbox around a codebase. That is the work of T3 Code, Claude
Code, Codex and the rest, and they are better at it than Ghost will be. Ghost's own
history points somewhere else. The original thesis ([original/prd.md](original/prd.md))
was "turning a scattered backlog into decisions that unblock engineering". This document
widens the backlog from GitHub issues to anything the operator wants observed, and adds
what Ghost was missing: noticing on its own, and reaching the operator when it matters.

## What a chief of staff does

1. **Knows the state of everything.** What ran, what broke, what changed, what is waiting
   on the principal.
2. **Protects attention.** Most things can wait for a digest; some are worth a tap on the
   shoulder; a few are worth a phone call. Deciding which is the job.
3. **Brings decisions, not raw material.** "The nightly build failed on the migration
   step; want me to open an issue?" rather than a log.
4. **Carries the answer back.** The operator says yes; the chief of staff makes it happen
   or tells whoever should.
5. **Does real work.** Drafts the memo, builds the deck, writes the summary.
6. **Follows up.** "Tell me if that deploy isn't green by five."
7. **Keeps the record.** What was decided, by whom, when (the audit already does this).

Ghost already does 3, 4 and 7 for GitHub issues by voice. 1, 2 and 6 are new, and they
are most of this document. 5 is about where Ghost's own work stops.

## What Ghost makes, and where it stops

**Ghost does work itself.** Asked for a presentation, it writes the HTML and JavaScript
page and hands it over; asked for a summary, a plan, a document, a small script, a
spreadsheet, it makes it. It does not hand that off to another tool just because the
output is code.

**Ghost is not a software development harness.** It does not work inside a codebase the
way an IDE agent does: checking out a project, editing its source across files, running
its build and tests in a loop for an hour, authoring branches and pull requests. When the
operator wants that, Ghost can hand it to the tool built for it and observe how it goes,
like any other system.

The line is not "code or not code". It is: a self-contained thing Ghost makes is Ghost's
work; sustained engineering inside a project is a harness's.

Two rules carry over unchanged. What Ghost makes is written through the gate, into a
place the operator chose (a department's folder, an output folder), with an audit row
like any write. And Ghost does not run what it makes: a script it wrote runs only through
the gated scripts server (`ghost-scripts`), and a page it built is opened by the
operator.

## Always listening, always observing

Ghost's passive state is the model for everything new here. At the desk the mic is
always on, but only small local models listen, nothing is kept, and the cloud model is
reached only after a wake phrase fires (ADR 0010). Observing should work the same way:

| | Listening (today) | Observing (proposed) |
|---|---|---|
| Always on | VAD and Parakeet on the mic | Deterministic checks and incoming notifications |
| Cost while idle | Local CPU | Local CPU, a few reads a minute |
| Model involved | None until the wake | None until a rule says to act |
| What fires | A wake phrase | An observation matches one of the operator's rules |
| What happens then | A session opens | Ghost records it, tells the operator, or acts |

Most "did something happen?" questions have deterministic answers: is the build red,
is the disk over 90%, did a message arrive from this sender, did a job finish, is
something waiting on me. Nothing about noticing needs a model, so nothing about it should
cost tokens, leak context or hallucinate.

The general rule: **deterministic first, the model last.** Everything between an outside
system and the model is deterministic code that can be tested without a model: verify,
parse, constrain, check at the source, match the operator's rules. Only what survives is
handed to the model, and only as typed fields. A parser or a rule is never replaced by a
model because the model would be quicker to write.

Contrast OpenClaw's heartbeat, which wakes the model every thirty minutes with a
checklist and drops its answer when the model says nothing needs attention. It works,
but it spends a model call to learn "nothing happened", and it is only as reliable as the
model reading the checklist.

### Two ways in: polled and pushed

A **watch** is something the operator declares in a department folder (ADR 0024), never
written by the model. It is one of two kinds:

- **Polled.** Ghost asks on an interval. Usually an MCP read the department already
  declares (ADR 0022) plus a small predicate over the result: "workflow runs on
  heka-vault/ghost with conclusion `failure`", "pull requests where my review is
  requested". No client code per service: if a system has an MCP server, it can be
  watched.
- **Pushed.** The system tells Ghost. Anything that can send an HTTP request can post to
  a signed endpoint on the daemon: a cron job, a monitoring alert, a script, a coding
  tool's hook. This is the same intake #10 plans for routines' webhooks.

Each observation becomes an **item** with a stable key (so the same failure is one item,
not one per check), its source, a short title, when it started, and a link to open it.
Ghost diffs what it sees: a new key is news, a key that disappears is resolved, a key
that stays is a candidate to escalate.

Whatever an observation carries is untrusted data, like issue text and MCP results. It
names an item and carries a title; it never changes a level, a scope, a prompt or a
rule.

### What Ghost does with an observation

The operator's rules, in the same folder, say what each watch's items get. The model
picks none of this:

- **Record.** Kept for the digest and for "what happened overnight?" Nothing else.
- **Tell.** The item goes on the **waiting list**, the list of things that need the
  operator, and Ghost reaches them by the routes below.
- **Act.** A routine (#10) runs in the department: the model gets the item as data,
  with the department's servers and skills, and does what the routine says ("read the
  failed job's log, summarise it, open an issue"). Its writes go through the gate at the
  department's levels; at `confirm` they wait for the operator, which puts them on the
  waiting list. Tell and act can go together.

So the model is reached only at **act**, or when the operator talks to Ghost.

### The waiting list

The waiting list is the part of all this that is about the operator. It holds what came
from **tell** rules, and Ghost's own items: a change at `confirm` with no yes, a server
that needs a secret, an agent that failed (issue #11's "waiting on me"). An item leaves
it when it is resolved at its source.

"Ghost, what needs me?" is answered by reading the list, not by sending a model off to
search everything, so the answer is fast and doesn't skip whatever the model forgot to
check.

### Examples of what could be observed

| System | Polled or pushed | A rule might |
|---|---|---|
| CI (GitHub Actions) | Polled through the GitHub server already declared | Tell on a red main; act to summarise the log |
| Pull requests and issues | Polled through the GitHub server | Tell when a review is requested |
| Servers and services | Pushed from a monitoring alert or a cron check | Tell, escalating to a call if it stays down |
| Messages to the operator (Bird SMS or WhatsApp, email) | Polled through the provider's MCP server | Tell when a named sender writes |
| Coding tools (T3 Code, Claude Code, Codex) | Polled through T3's MCP server; pushed from Claude Code or Codex hooks | Tell when a thread is waiting or failed |
| Long jobs (a training run, a render, a backup) | Pushed by the job when it ends | Record on success, tell on failure |

None of these needs Ghost to grow code for that system. A system with an MCP server is
watched through it; a system without one pushes to the endpoint.

### Pushed from outside the house

Anything on the workstation, the LAN or the operator's WireGuard network posts straight to
the desk. Hosted services (GitHub, Bird, a SaaS alert) send webhooks from the internet,
and ADR 0027 (accepted, amending ADR 0020) allows a public listener for them, in the
daemon, on strict terms:

1. **Signature first.** The body is not read until the source's signature, clock window
   and delivery id check out.
2. **Parsed by declaration, not by a model.** The department declares which events a
   source sends and which fields to keep, each with a type and a constraint (a pattern,
   an allowlist). Anything else in the body is dropped at the door; a body that doesn't
   fit is rejected whole.
3. **Checked at the source.** A webhook that says run 123 failed is a hint: Ghost reads
   run 123 from GitHub through the declared read, and the read is what it believes.
4. **No raw text to the model,** and a webhook can never write, confirm or change a
   permission. It produces an observation; the operator's rules decide what happens.

Polling stays the fallback for anything that can't meet those terms.

## Reaching the operator

Once something is on the waiting list, Ghost has to decide how loud to be. This is the
part most likely to go wrong, and the operator already suspects it ("it might be calling
frequently").

### Delivered is not acknowledged

An item leaves the waiting list only when it is resolved at its source (the build went
green). It stops alerting only when the operator **acknowledges** it. Sending a
notification, or saying something at the desk, is neither. A route either gets an
acknowledgment or hands the item to the next route the operator has enabled.

What counts as an acknowledgment, per route: a spoken reply at the desk; opening the
notification in the app; answering a call and staying on it past the greeting; a reply
to a message. A route that can never be acknowledged (an SMS sent while the operator
ignores it) is the last on its ladder.

### Routes, quietest first

Every route except the digest and the app's notification is **off until the operator
enables it**, in the policy file or the desktop's settings. The model never turns one
on.

1. **Digest.** Held until the next wake ("Before you ask: the nightly build failed and
   one review is waiting on you") or a scheduled summary. Always on.
2. **Desk voice** (opt-in). Ghost chimes and greets, and says nothing about the item
   yet ("Eric, got a minute?"). It waits a short window (15 s, say) for the operator to
   answer. A reply is the acknowledgment: Ghost then says what is waiting and the session
   carries on like any wake. "Not now" snoozes it. No reply means nobody heard, so the
   item is not acknowledged and goes straight to the next enabled route, without waiting
   for the escalation timer. Withholding the content until the operator answers also
   keeps it from a room the operator isn't in. While Ghost waits for the reply, the mic is
   under the passive rule: local STT only, nothing kept or sent until the operator
   answers. Desk voice does not interrupt a running session or a hold; the item waits for
   the session to end, or goes to the next route.
3. **Desktop notification** (opt-in). Clicking it is the acknowledgment.
4. **Phone notification.** Through Ghost's own phone app; opening it acknowledges.
5. **App call** (opt-in). The phone rings with a full-screen incoming call. Answering
   opens an ordinary phone session (ADR 0016) in the agent that has the item, over the
   operator's tunnel (ADR 0020), and Ghost opens with the question. The operator answers
   by voice, and Ghost carries the answer back through the gate.
6. **Carrier routes** (opt-in): SMS, WhatsApp, and a real phone call through a
   communications provider such as Bird (see below). These reach the operator without
   the app or the tunnel, at the cost of sending something through a vendor.

The app call is the strongest idea here because it is the only route where the
notification is also the place you answer. Everything it needs exists already: the phone
app, the WebRTC listener, agents, the gate, voice confirmation. What is new is that the
desk starts it.

### Carrier routes through Bird

The operator uses Bird (formerly MessageBird) in another project. Checked 2026-10-27
against Bird's docs:

- **Bird has an official MCP server** (hosted at `mcp.bird.com` over streamable HTTP, or
  `bird mcp` over stdio), so it can be declared like any other server (ADR 0022), with no
  Bird client in Ghost. Its tools include `sms_send` and `whatsapp_send`, and reads of
  incoming messages, so Bird is both a route out and something to observe.
- **SMS and WhatsApp** through it look ready to use as routes.
- **Voice exists, but not unattended through MCP.** Bird's Voice API places outbound
  calls (`POST /v1/voice/calls`) with a call flow that can speak text (TTS) and take
  keypad choices. The MCP server's `voice_calls_create` only *prepares* a call; a person
  has to run it in Bird's web app, so Ghost cannot place a call through the server alone
  today. Placing one unattended would mean the REST API, which ADR 0022 says to reach
  through a server, not through code in the daemon.
- **What a Bird call could be.** The simple version is one-way: Bird rings the operator,
  speaks "Ghost here: the nightly build failed", and "press 1" acknowledges. A two-way
  conversation with Ghost over the phone network would mean bridging Bird's call audio
  into Ghost's pipeline (SIP or a media stream), which needs an address Bird can reach.
  ADR 0027 covers webhooks only, not call audio, so that needs its own decision.
- **Auth to check.** The hosted server signs in through the browser (OAuth); the docs
  are unclear whether the stdio server accepts a `BIRD_API_KEY` from the environment,
  which is what Ghost's secrets rule needs for an unattended route.
- **Which account.** If the Bird workspace belongs to the other project, Ghost using it
  sends the operator's personal notifications through that project's account and bill.
  A separate workspace for Ghost may be cleaner.

Sending a message or placing a call is a write to an outside service, so it goes through
the gate like any declared write: the operation, its level, its audit row. The
difference is that the daemon starts it, not the model, and the operator's route policy
is what asks for it.

### An interruption budget

The operator's policy, not the model, decides the route, like autonomy levels do for
writes (ADR 0011). A sketch of what that policy holds:

- **Urgency per watch:** `digest`, `notify`, `call`, with `notify` as the default.
- **Escalation:** move up one route if still waiting after N minutes, and never past the
  watch's ceiling.
- **Presence:** at the desk (a session ran recently, or the desktop is in use) prefers the
  desk; away prefers the phone.
- **Quiet hours** and a do-not-disturb the operator can set by voice ("Ghost, nothing
  until two").
- **Batching:** items within a short window go out as one.
- **A hard cap on calls** per hour and per day; past it, everything drops to notify.
- **Acknowledge and snooze:** an item the operator has acknowledged does not alert again
  until it changes. An item that was only delivered keeps escalating.
- **Enabled routes:** the ladder skips every route the operator has not turned on.

### Privacy and the outside world

Today only LLM calls and server calls leave the machine. Telling the operator things
adds a new kind of traffic, and the rule should be stated before the code exists:

- The default route is Ghost's own app over the operator's tunnel. A push service that
  wakes the phone (FCM, or a self-hosted UnifiedPush/ntfy on the desk) carries no
  content, only "check in"; the phone fetches the details over WireGuard.
- A third-party messenger receives only what its channel's policy allows: by default the
  source and a count ("CI: 1 failure"), not titles or text.
- A call never goes through a telephone carrier unless the operator enables a carrier
  route; the default call is the app's WebRTC session. A carrier call speaks only what
  its route's policy allows, by default the same source and count.

## Acting back on other systems

Observing is half of it; the operator's answer often has to go somewhere. "Yes, open the
issue", "tell the T3 thread to go ahead", "restart the service", "reply that I'll call
tomorrow". Each of those is a write to an outside system, so it is a declared operation
through the gate (ADR 0022), with its level in the operator's policy and its audit row.
Answering another system on the operator's behalf is exactly the kind of write `confirm`
exists for.

## What the field is doing

The operator asked whether this is where Grok, Manus, OpenBot and the like are heading.
Mostly yes:

- **Grok** added Tasks (scheduled prompts with results by notification, 2025) and
  Automations (schedule or email triggers, July 2026). Proactive suggestions are reported
  but unconfirmed.
- **Manus** went from one-off tasks to persistent agents in Telegram (February 2026), an
  always-on cloud computer (April), and event-driven Automations triggered by email,
  calendar, Slack and the like (Manus 2.0, September 2026).
- **OpenClaw** is a local gateway with every messenger plugged in and a heartbeat that
  wakes the model on a timer to check a list.
- **OpenBot** has routines on a schedule or a signed webhook (reviewed for #10).

The common direction is a persistent agent that runs all the time, is triggered by events
or a clock, and reaches you where you are. Where Ghost differs, and should keep
differing:

- **Observing without a model.** Their always-on means a model on a timer, or a model
  run per event. Ghost's means deterministic checks and the operator's rules, the way its
  passive listening is local; the model is reached only when a rule says act.
- **Makes things, but is not an IDE.** It does real work, short of running sustained
  engineering inside a codebase.
- **Voice first, and a call you can answer.** None of them treat a phone call as the
  primary way to reach you and settle the question in the same breath.
- **Bounded writes.** Their permissions are broad by design (OpenBot calls its own "not a
  security boundary"). Ghost's gate stays.
- **One operator, one machine.** No cloud computer to rent.

## What changes in what is built

Nothing already built is thrown away. The gate, MCP declarations, departments, agents, the
phone app and the tunnel are all what this needs. What moves:

- **GitHub triage becomes one department**, the first and best example of the pattern,
  not the product.
- **#11 (status)** becomes the waiting list plus Ghost's own state. It moves up from
  backlog into the next increment.
- **#10 (routines)** becomes the **act** half of a rule, triggered by an observation as
  well as by a schedule.
- **New ADRs, in order:** the public listener for webhooks (ADR 0027, accepted); watches
  and observations (polled, pushed, the local endpoint); rules (record, tell, act) and the waiting list; reaching the operator
  (routes, the interruption budget, the privacy rule for outbound messages);
  Ghost-initiated sessions at the desk and on the phone; what Ghost makes and where it
  writes it.
- **New words for GLOSSARY.md:** *watch* (what the operator declares to observe, polled
  or pushed), *item*, *rule*, *waiting list*, *route* (how Ghost reaches the operator).

## Open questions for review

1. **Is observation the core of Ghost?** If yes, "what needs me?" is answered from the
   waiting list alone, and voice triage of GitHub is one department among many.
2. **The predicate language for polled watches.** Smallest useful: a field, an operator
   (`in`, `exists`, `>`), a "for at least N minutes". Anything larger should be a
   script, which raises whether watch scripts run through `ghost-scripts`.
3. ~~Ghost speaking first at the desk.~~ Decided 2026-10-27: opt-in; greet, wait for
   a reply, deliver only then; no reply passes the item to the next route (see Routes).
4. **Phone wake-up.** FCM with no content, or self-hosted UnifiedPush/ntfy on the desk
   over WireGuard? The second keeps Google out entirely but needs a long-lived
   connection on the phone.
5. **Carrier routes.** Decided 2026-10-27: opt-in, Bird as the first candidate. Still
   open: the Bird workspace to use, whether the stdio server takes an API key, and
   whether an unattended Bird call is worth a small server around the Voice API, given
   the MCP server only prepares calls.
6. **Which messengers, if any, beyond Bird's SMS and WhatsApp.** Telegram is the easiest
   bot API; Signal is the most private.
7. ~~Webhooks from hosted services.~~ Decided 2026-10-27: a public listener is allowed
   on ADR 0027's terms (signed, parsed by declaration, checked at the source, no raw
   text to the model, tested with recorded deliveries), and it runs in `ghostd` on its
   own addresses.
8. **Answering another system by voice.** At what level by default: `confirm` for
   everything, or lower for low-risk replies like "go ahead"?
9. **Where Ghost's work goes.** Decided 2026-10-27 that Ghost makes self-contained
   things itself, code included, and is not an IDE. Still open: where they land (a
   department's `docs/`, an `out/` folder), how the operator gets them by voice ("it's
   on your desktop", a link to the phone), and whether the desktop app previews them.

## A first increment

Small enough to live with for a week and learn from, and built on a server Ghost already
has:

1. One polled watch: failed workflow runs on heka-vault/ghost, through the GitHub server
   already declared, every minute.
2. The pushed endpoint, signed, local and tunnel only, with one sender: a shell one-liner
   the operator can put at the end of any long job.
3. Rules with record and tell only (act waits for routines).
4. The waiting list in the store, with Ghost's own pending confirms in it, and "what needs
   me?" answered from it.
5. One route: a phone notification through the app, with batching, quiet hours and
   acknowledge.
6. Measure: how often it fired, and how often the operator wished it had or hadn't. That
   is the data for whether desk voice and calls are a good idea.

Then desk voice and the app call, then act rules through routines, then more watches
(Bird messages, T3 threads, monitoring) as the operator wants them.

## Sources

- [Grok Automations (July 2026)](https://imisofts.com/blog/grok-automations-scheduled-agents-news-july-20-2026/), [Grok Tasks](https://grokipedia.com/page/Grok_Tasks), [unconfirmed proactive suggestions](https://supergrok.online/grok-bot-proactive-task-suggestions/)
- [Manus 24/7 agents via Telegram](https://www.testingcatalog.com/manus-ai-launched-24-7-agent-via-telegram-and-got-suspended/), [Manus 2.0 Automations](https://www.explainx.ai/blog/manus-2-0-studio-cue-cascade-cloud-computer-2026), [Manus Cloud Computer](https://aiautomationglobal.com/blog/manus-cloud-computer-always-on-ai-automation-2026)
- [OpenClaw heartbeat docs](https://open-claw.bot/docs/gateway/heartbeat/), [OpenClaw architecture](https://dev.to/entelligenceai/inside-openclaw-how-a-persistent-ai-agent-actually-works-1mnk)
- OpenBot: reviewed 2026-10-25, see issues #10, #11, #12
- Bird: [MCP server](https://bird.com/docs/ai/mcp-server), [Voice API](https://bird.com/en-us/developer/voice-api), [Say (TTS) during a call](https://docs.bird.com/api/voice-api/voice-calls-api/say-text-to-speech-tts), [voice pricing](https://bird.com/en-us/pricing/voice)
