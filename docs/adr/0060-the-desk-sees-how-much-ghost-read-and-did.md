# 0060: The desk sees how much Ghost read and did, a day at a time, as it happens

- **Status:** accepted
- **Date:** 2026-11-12
- **Amends:** ADR 0013 (one more read, two more events, two date filters), ADR 0047 (a run is told as it starts and ends), ADR 0058 (activity has a second, coarser record across sessions and runs)

Today (docs/design-system.md, Today) could not say how much Ghost read: reads were recorded
nowhere the app could see. A past day paged back through the whole audit, and the app asked for
the runs every 20 seconds while one was going. The owner's bar is a screen that feels like
wielding a supercomputer, earned with true numbers, so the daemon now keeps the numbers.

## Decision

**A `reach` row per tool call an agent makes** (`ghost_core::reach`, the store's `reach` table):
the time, the session or run (`run-<id>`), the agent, the app, the tool's name as the app calls
it, whether it was a read, whether it came back without an error, how many items, how many
bytes and how many milliseconds. Never contents: no arguments, no result, no words, no error
text; the table has no column that could hold them. The row is written by the one place every
call of a session and a run passes, `ghost_voice::tools::ToolDispatcher::call`, through the
gate's store (`WriteGate::record_reach`, `spawn_blocking`, off the turn). The session's own
tools (hang up, hold, a transfer) are not calls to an app and are not kept. An outside tool's
calls (ADR 0048) reach only Ghost's own app and are not kept here; a watch's poll and a
schedule's check (no model) are not kept either, so the number is what agents read.

- **Bytes** are the app's answer as it arrived, after redaction and before it was fitted to the
  model's budget (ADR 0046): what Ghost read, not what the model was shown. `App::attempt`
  measures it (`reach::note`) into a task-local the dispatcher opened (`reach::measured`); a
  change the gate ran on another task is measured from the result the model got instead.
- **Items** are deterministic (`reach::items_of`): a list's length, the length of the one list a
  small wrapper carries (`{"items": [...], "total_count": 3}`), else 1; none for an empty
  answer or a failure.
- Kept 90 days (`REACH_KEPT_DAYS`), pruned with the daily transcript prune.

**`GET /reach?since&until&sessions`**: the calls in a stretch of time (at most 31 days), totalled
(calls, reads, changes, failed, items, bytes, time, apps), by app, by agent and by session or
run. Today says "read 412 items across 4 apps, 3.7 MB" from it, and the timeline shows a run's
reach.

**`since` and `until` on `GET /audit` and `GET /runs`** (RFC 3339): a day of the timeline is
read by its range (`created_at`, newly indexed; a run's `scheduled_for`), so a past day no
longer pages back through everything.

**`Event::Run { run, phase }`** when a run starts and when it has ended, with its row as it is
then (`Scheduler::on_run`), and **`Event::Reach { reach }`** for each call as it is kept: the
run's progress, and a session's. The desktop follows the work from these and stops re-reading
every 20 seconds. Neither goes to the phone.

## Cost on a turn (ADR 0018)

Measured on the workstation, release build:

- On the call's path: one more serialisation of the app's answer to count its bytes, about
  0.1 µs for a small answer, 31 µs for 36 kB (500 items), 0.3 ms for 368 kB. Plus a lookup of
  the tool's app in the offered list, a few microseconds. Nothing else waits.
- Off the path: one SQLite insert per call, about 2.4 ms on the WAL store, on a blocking thread;
  it holds the store's connection for that long, so a turn writing at the same moment can wait
  up to that much. A turn with two calls adds two. One event per call, built only when someone
  is listening.
- `GET /reach` for a day of 5,000 calls: 2.6 ms to read and total.

## Consequences

- The size of what Ghost read is now a fact on screen. A hostile app can inflate it (a large
  answer), which costs it nothing it could not already do; the number is never read by a model.
- Anything that wants a call's contents still goes to `GET /calls/{id}` (ADR 0058): bounded,
  redacted, memory only, the live session's last 64.
