# 0047: Agents work on a schedule, with nobody there

- **Status:** accepted (decided 2026-11-11; built on the `feat/schedules` branch: the run with nobody there, the scheduler and its table, a watch's `act`, the two tools on Ghost's own app, the control API's `/schedules` and `/runs`, and the desktop's Watching screen; a run keeps up with the apps as ADR 0046 has a session do)
- **Date:** 2026-11-11
- **Settles:** issue #10 (routines), and the `act` response ADR 0029 left proposed
- **Amends:** ADR 0028 and ADR 0029 (a watch may act), ADR 0030 (a change at `ask` from a
  run is set aside at once), ADR 0045 (two more tools on Ghost's own app), ADR 0018 (their
  cost on a turn, and what an idle daemon with schedules costs; both below), ADR 0034 and
  ADR 0035 (schedules are in the database, not a file in Ghost's folder)

Nothing ran unless a voice session was open. Ghost could notice a red build (a watch, ADR
0028) and tell the operator, but no agent could look into it, and "check the build every
morning at nine" had nowhere to live. A watch's `act` response was refused by the parser.
OpenClaw and Hermes Agent both have this, as cron jobs that run an agent turn and deliver
the result; this ADR takes their shape and keeps Ghost's rules.

## Decision

**A run is a session's model and tools without the session** (`ghost_voice::headless`). It
runs Ghost or a named agent on an instruction: the agent's own prompt (the rules, its
persona, the apps' paragraphs) written for reading instead of speaking, its memory, notes
and recent turns as the context, the model through `ghost-llm`, and every tool through the
session's own dispatcher and so the same gate. There is no audio in it, and none of what
exists for audio: no narration cut, no spoken filter, no hang up, hold, transfer or
bringing up a change. It loops until the model stops calling tools, for 30 model calls at
the most (the last is offered no tools, so a run ends with an answer) and ten minutes at
the most, and it can be stopped. The tools are asked for before every model call, as a
session asks (ADR 0046), and a run that is stopped or out of time stops its calls to apps:
each app is told, each call still returns, and a change still has its outcome in the
audit. A tool that does not stop when told is left behind after five seconds. A model that cannot be reached is tried again after 30,
60 and 120 seconds, then the run fails.

**A schedule is one job**, a row in SQLite (`schedules`): a name, the agent, the
instruction, when, whether it is on, who made it (`operator` or `agent:<slug>`), its last
and next run and how the last one ended. When is one of three kinds: `at` a time, `every`
interval, or a `cron` expression of five fields in a named time zone (the machine's by
default), evaluated by `croner` over `chrono-tz`. A job may carry active hours, a grace
window, a time and turn limit, skills to start with, how loud its answer may get, and a
check.

**The scheduler** (`ghost_core::schedules::Scheduler`) looks every 15 seconds and when
something is queued. It claims what is due, runs one job at a time per agent, and records
a run's row before the run starts.

- **After Ghost was off**, a schedule runs once if its time was missed by less than its
  grace window (two hours unless set), however many times were missed, and not at all
  after that. There is never a backlog of runs.
- **A run that recorded its result is neither run nor delivered again after a restart.**
  One that was going when Ghost stopped is marked interrupted, told once as a failure, and
  not run again: Ghost does not know how far it got.
- **Paused** means it keeps its time and does not run; resumed, it counts from now.

**The result reaches the operator through what exists.** A run's final answer is an item
on the waiting list under `agent:<slug>` (as `ghost_notify` makes, ADR 0045), the answer
as its detail, as loud as the job says, so the routes deliver it with their quiet hours,
batching, acknowledgment and escalation. A schedule has one item: the next answer replaces
it and alerts again. An answer of exactly `[SILENT]`, or an empty one, delivers nothing,
and the run says why. A run that fails delivers a failure item unless the job switches
that off. If the agent called `ghost_notify` itself during the run, its answer is not
delivered as a second item. The answer is also written into the agent's transcript, marked
`[scheduled run: <name>]` under the run's own session id, so the agent has it when the
operator asks what the morning check said. A run with nothing to tell is one line of the
transcript that the model's context does not carry.

**A check runs first, with no model** (Hermes Agent's monitor mode, and Ghost's own
deterministic-first rule). A job may name one of a connected app's read-only tools with
its arguments. The scheduler calls it through the watches' read path, hashes what it
returned with its keys in order, and runs the agent only when the hash differs from the
last run's, giving it the lines that changed. A check that fails is a failure, never a
change. A change the agent failed on is still a change next time.

**A watch may act.** `"response": "act"` with an `act` block (`agent`, `instruction`)
starts a run for each new item, given the item's typed fields as data: the watch's name,
the key, the title, the link and the kept fields, nothing else the app sent. The parse and
re-read rules that made the item are ADR 0027's and ADR 0028's, unchanged. A watch acts
once per item and twelve times an hour at the most, unless its block says otherwise. The
operator declares it in `watches.json`; no Jev or model step can choose it. A webhook
(`hooks.json`) does not act in this increment.

**Agents manage schedules through Ghost's own app**: `ghost_schedules_list`, a read, and
`ghost_schedule_manage` (create, update, pause, resume, remove, run_now), a change through
the gate. The calling agent is the dispatcher's `_agent`, never an argument the model
writes. A schedule an agent makes is attributed to it and runs as it; an agent manages its
own, and Ghost manages all. The operator's own way in is the control API (`/schedules`,
`/runs`) and the desktop's Watching screen.

## What a run may and may not do

- It may do what the agent may do in a session: call every connected app's reads, and ask
  the gate for any change. The operator's `block`, `ask`, `quiet` and `allow` rules apply,
  and every change is in the audit under the agent and the run's session id (`run-<id>`).
- **A change that resolves to `ask` is not made.** Nobody is there to say yes, and nothing
  in a run is the operator's words: the instruction is never read as the request that
  plainly asks for a call (ADR 0017). The change is set aside on the waiting list as ADR
  0030 does for one a session left unanswered, and the run carries on and says so.
- It cannot add a rule (`rules.add` needs the operator's own words), set a permission, or
  write `apps.json`, `watches.json`, `hooks.json` or `policy.toml`. It cannot reach the
  control API.
- It never opens the microphone or the speaker, and shares nothing mutable with a session
  that is open beside it: its own context, its own dispatcher, no listener on the event
  bus. Idle audio is untouched.
- What a check, a watch's item or an app returns is data: it is given to the model marked
  as such, and none of it changes a permission or a prompt. Passwords are redacted from
  what a run stores, as everywhere.

## Consequences

- **Schedules are the first thing the operator owns that is not a file in Ghost's
  folder.** Agents create them, and the model never writes a file there except through
  `rules.add`; a table with `created_by` keeps that rule whole. `ghostd check` lists them
  and names one that does not read or names an agent Ghost does not have, and so does the
  waiting list, as for a file with a problem.
- **A schedule made in the desktop is not a gate change.** It is the operator's own edit
  of Ghost's bookkeeping, like acknowledging an item (ADR 0029). One made by an agent is.
- **Cost** (ADR 0018). The scheduler's look is two SQLite reads every 15 seconds and no
  network. A model is called only when a job is due, and with a check only when the check
  changed: an unchanged check costs one read of an app and no model call (asserted:
  `a_check_that_returns_the_same_thing_runs_no_agent`). On a turn, the two tools add
  **2 tools and 2,713 bytes** of JSON schema (about 680 tokens) to every session's cached
  prefix (`what_the_schedule_tools_add_to_every_turn`; the test fails past 3,200 bytes).
  `ghost_schedule_manage` is 2,286 of them: one tool with an action, not six.
- A run takes the model connection a session uses. A run and a session may call the model
  at the same time; neither waits for the other.
- Two new crates: `croner` (MIT) for cron expressions and `chrono-tz` (MIT or Apache-2.0)
  for named time zones; `iana-time-zone` was already in the tree. All pass `cargo deny`.

## What follows

- A webhook that acts, with the same block.
- An agent that hands a run to another agent, and a run that waits for the operator's
  answer (the `ask` tool ADR 0045 names).
- Per-job model choice, when there is a second connection (ADR 0037).
- Editing a schedule's time and options in the desktop; the control API takes them now.

## Alternatives considered

- **A heartbeat: wake the model on a timer and let it decide** (OpenClaw has both). It
  spends tokens to learn nothing happened, which ADR 0028 already refused. A schedule with
  a check is the same idea with the deciding done by a hash.
- **Schedules as `schedules.json` in Ghost's folder.** The operator could edit it by hand,
  but an agent could not create one without the model writing a file there (ADR 0035), and
  a run's state would need a second place anyway.
- **Let a run confirm its own change, or treat the instruction as the yes.** The
  instruction was written once, perhaps by an agent; the change it leads to weeks later is
  not something the operator asked for in so many words. `ask` means a person answers.
- **Run inside an open session when there is one.** The operator would hear a morning
  check in the middle of a sentence. A run is silent; what it found waits for a route.
