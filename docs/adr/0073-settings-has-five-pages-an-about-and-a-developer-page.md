# 0073: Settings has five pages, an About and a developer page

- **Status:** accepted
- **Date:** 2026-11-14
- **Amends:** ADR 0014 and ADR 0061 (an agent's settings are on its own page, out of Settings),
  ADR 0036 (the list of passwords moves under Developer; every password is added and replaced
  where it is used), ADR 0048 (Outside tools under Developer), ADR 0051 (how Ghost reaches you is
  a short section of General, the routes in detail under Developer), ADR 0066 (updates are on
  About), ADR 0067 (the kept reports are part of a diagnostic report)

The owner approved a simplification pass after a review of the best assistant apps: Ghost's
Settings had eleven pages (General, Model, Agents, Passwords, Skills, Permissions, Sound,
Phone, Outside tools, Notifications, Audit log) where the field has one dialog. The target was
about five pages a newcomer sees, nothing an advanced operator can do today removed, and fewer
nouns for a newcomer. The owner then asked for "an about part of settings, with the version
number, and being able to click the number a few times to get access to advanced developer
level settings", to "keep testing on the laptop [with a release build] but also record and
share back the results".

## Decision

**Five pages for everyone: General, Model, Sound, Permissions, About.** General is how Ghost
starts and shows itself (Set up Ghost, Start at login, where Ghost appears, how Ghost reaches
you, connections, error reports, Remove Ghost from this computer); Model and Permissions are as
they were (Permissions leads with "What you've decided", ADR 0070); Sound is as it was; About is
new. Each old page's address redirects to its new home (`REDIRECTS` in `app/routes.tsx`), and
`screens/settings/moved.ts` lists every control there was and where it is now, held by a test.

- **Agents** leave Settings. The roster is `#/agents` (the rail's agents and its + lead there);
  an agent's settings are its own page, `#/agents/<slug>/settings`, reached from its
  conversation. Everything on the old page (the roster, Switch, New agent, Ghost's rules, Open
  folder, what Ghost made) is on the roster, so the Settings page went.
- **Passwords** are asked for where they are used: the model page (as before), Sound's realtime
  voice and an app's page now carry the vault's own row (`PasswordInPlace`), so a key is added,
  replaced or deleted there, with "Stored in Ghost's vault · Manage". The list of names (never
  values), what uses each and when it was last used, the unlock and the import are
  Developer › Passwords; the locked-keyring note still leads straight there.
- **Notifications**: "How Ghost reaches you" on General, in a sentence, with the three everyday
  routes (asks at the desk, a desktop notification, the phone app) as switches. How long before
  the next way, quiet hours, limits, ntfy and the greeting are Developer › Notification routes,
  linked from that section. Telling Ghost where to send updates by voice is a later step.
- **Phone**: General › Connections › Your phone; **Pair a phone** opens the pairing dialog
  (`#/settings/phone/pair`): Settings, General, Pair a phone.
- **Skills, Outside tools, the Audit log**: Developer's. The audit's content is Today's timeline
  of changes; the full log is at `#/settings/developer/audit`.
- **Where Ghost appears** is one choice in General: floating over your work, or in the window,
  with the floating choices (monitor, placement, captions, while shared) under it. Ghost on
  stage is the tray's.
- **Start at login**, the tray's only until now, is a switch in General too (the shell's
  `login_status`/`login_set`, the same file the tray writes).

**About**, for everyone: the version and build and this window's (the rows General had), the
updates (`UpdatesSection`, moved, the same state), the site and the README's Privacy section,
the licences of what Ghost is built from (`apps/desktop/public/licences.json`, written by
`tools/licences.py` from Cargo.lock through `cargo metadata` and the desktop's production
packages through `pnpm licenses`; a test says when it is behind Cargo.lock), and **Save a
diagnostic report**.

**The diagnostic report** is one JSON file in the person's Downloads folder (`XDG_DOWNLOAD_DIR`,
else `~/Downloads`, else home), 0600, never over another file, built by the daemon
(`ghostd::diagnose`, `POST /diagnose` and `POST /diagnose/preview`, `ghostd diagnose`) from what
Ghost already knows: version, build, how it was installed, the OS and session type, whether it
can float (the display, and the shell's own line), the sound devices and how Ghost hears (the
case ADR 0064 decides, so whether the input is a tested array), the model's provider and model
names and its last check's timings, the speech models, the echo canceller, the updater's line,
the last 30 turns' timings (numbers and times only), the kept wake attempts (counts and scores
only), the kept error reports exactly as sent, the apps by name and state and how many agents
and schedules, and Ghost's last 400 log lines. Every string but the error reports (scrubbed
when made) passes `ghost_reports::Scrubber` with the vault's redaction first; a log line, an
error and a problem keep only the code's own words (`scrub::head`), so an app's text after the
first `": "` goes. It never holds what was said or typed, audio, a password, an app's content,
a persona or memory, a home path or an address's credentials: tests feed each in as a hostile
fixture. Three boxes, unticked, add the person's own: the last 40 turns with Ghost, debug mode's
log while it is on (ADR 0074), and up to eight kept wake recordings; each says what it adds, and
the summary before saving lists every part with its size and marks theirs. Nothing is uploaded:
the person decides who gets the file. Debug mode's log leaves out of Ghost's log tail (the
desktop's log mirror and a report's) whatever `ghost_voice::debug` logs, so the person's words
reach a report only by its box.

**Developer**, shown once the version on About is clicked seven times (Enter and Space count;
from the fourth a hint says how many are left, in a live region), remembered on this computer
until hidden on that page. It is not a lock and says so: everything on it is a file or a command
the operator already has, and every page under it opens by its address. It holds debug mode
(ADR 0074); tests (the model check, `ghostd reports test` and `ghostd check` to copy, the first
run again, the floating position, the speech models, the version line, and "Record a test
session", which opens the Hearing test's recorder); the **experimental switches**; the advanced
pages (Passwords, Skills, Outside tools, the Audit log, Notification routes, Permissions'
Advanced, the settings file); and two slots marked "coming next" (a live timing overlay and a
live microphone meter, `DeveloperLive.tsx`, `#/settings/developer/live`) with no data until they
are built. (Built 2026-11-14: see the amendment below.)

**The experimental switches** are a closed list in code (`ghost_api::developer::SWITCHES`) of
settings that are environment variables, each shown by its real name with a sentence, its value
now and where that comes from (`settings.env`, the environment Ghost was started in, or the
default), written to `settings.env` by the same writer as every setting after a check of its
kind (`PUT /developer/switches`, which refuses any other name). They are read at Ghost's start,
so each takes effect at the next restart. Never a password, never `GHOST_DEBUG_AUDIO` (ADR
0073), never the voice mode (ADR 0063: the operator chooses it on Sound after what changes is
shown), never a listener's addresses or a path; the fast model's thinking and effort are left
out since the values a route takes are the provider's. A test holds that each names a variable
the daemon reads.

No tool of Ghost's own app, run, outside client, phone route, app or model reaches the
switches, debug mode or a report: the routes are the control API's (ADR 0013), and tests on the
phone's routes, the outside listener's tools and Ghost's own tools hold it.

## Consequences

- Settings' nav has five entries, six with Developer; the nouns a newcomer meets there drop
  from eleven page names to five.
- Two new daemon capabilities, both the operator's only: the diagnostic report and debug mode
  (ADR 0074). New reads for Developer: `GET /developer/switches` and `GET /debug-audio`. The
  shell gains `login_status` and `login_set`.
- `licences.json` is generated and committed; after a dependency change `make licences` writes
  it again, and `tools/tests/test_licences.py` fails until it does.
- A report can be large with wake recordings (base64 WAV, up to about 4 MB); without them it is
  tens of kilobytes.

## Alternatives considered

- **Connections as its own page** (Phone, Apps, Outside tools). It would have been a sixth page
  for a newcomer, and Outside tools is a developer's concern; a section of General does it.
- **A zip archive** for the report. One readable JSON file opens anywhere and needs no new
  dependency; recordings ride in it as base64.
- **Developer as a lock.** Nothing behind it is anything the operator can't do with a file or a
  terminal; a lock would be theatre, and hiding pages behind it would break their links.

## Amendment, 2026-11-14: the live views and the test buttons

The owner approved three things for testing release builds on a laptop: a live timing
overlay, a live microphone meter, and making "Send a test error report" and "Run ghostd check"
real buttons. All are on Developer; none is visible to a newcomer. The "coming next" slots are
gone.

**The live timing overlay.** A switch on Developer (off by default, remembered on this computer
in the app's own storage, and off whenever Developer is hidden) shows each turn of a live
session, small and monospace: under Today's live strip, under the conversation screen's stage,
and as one line under the floating Ghost's label (inside the layout rule's share, never a
target for a click or the keyboard). Its numbers are each turn's latency record (ADR 0018,
`ghost_voice::latency`, the record `ghostd latency` and the diagnostic report read), sent at the
turn's end as a `timing` event (`ghost_api::live::TimingView`, made by `timing_of`): the
person's turn closing, speech to text, the model's first word, the first tool call and its
result, the acknowledgment, Ghost's first sound, the reply's end, the model's name, the
provider and fast or main. The turn under way is drawn from the `activity` events the window
already has. The event is built only while the desktop's stream is open, off the turn's path
(after the reply ends): 4.7 µs a turn in a release build, measured (`what_a_turns_timing_costs`).
No stage was missing, so the pipeline is unchanged. **Copy the last ten turns** gives a
plain-text table; the diagnostic report keeps carrying the last 30 records and is not duplicated.

**The live microphone meter** (`#/settings/developer/live`, full width): the input level and peak
over the last eight seconds, the room gate's levels (the room, the person, the line under which
a segment is held, ADR 0072), whether the speech detector hears speech, and what Ghost decided
about each stretch of sound, by kind only: taken as the person's turn (with a word count), read
with no words, held as the room, let go, held then taken by Ghost's name, dropped as Ghost's own
echo, too short to interrupt, a barge-in, a wake attempt and its score (and whether Parakeet's
check let it wake Ghost), a near miss. The stages that already measure these report to
`ghost_voice::meter`, a process-wide collector that is one atomic load and nothing else while
closed; `ghostd::meter` holds a ten-second lease the page renews every four seconds and
publishes about fifteen `meter` events a second while it is held, then one saying it closed.
The page closing, hiding or going away ends it. On the on-chip array, where the room gate is
off, the meter says so and why. With listening off (ADR 0057) no capture is open; the meter
opens none and says there is nothing to measure. The phone's call is never metered.

**The rule for levels before the wake.** CLAUDE.md's "idle audio stays on the machine" and "the
control API never carries passive audio or passive transcripts" are kept as they were, and this
is how the meter sits under them. While the operator has the meter page open, and only then, the
passive listener's frame levels, the speech detector's state and the wake detector's scores and
decisions reach the operator's own desktop over the control API's event stream. They are
numbers and fixed kinds: not audio (one level per 20 ms frame, rounded to a tenth of a dB, is not
sound anyone can be heard in), not a transcript, and never a word. They stay on this computer, as
the desktop does. Nothing else subscribes to them: they are published only while the lease is
held, the phone's stream and the outside listener read a closed list of event kinds that does
not hold `meter` or `timing`, and no tool of Ghost's own app reaches the meter, the overlay, the
test report or the checks. No history is kept: each reading carries only what came since the one
before, the collector forgets everything when it closes, and the page keeps its last eight
seconds and forty decisions in memory only. The types have no field for text
(`MeterDecisionKind`, `TimingView`): `the_meter_and_timing_carry_no_words` and the speech-in
stage's hostile fixture (`test_the_live_meter_sees_the_room_held_and_the_person_taken_and_no_words`)
hold it. What is new is that passive telemetry reaches the desktop at all, while the operator is
looking at it; the alternative, a meter that shows nothing until a wake, would not show why a
wake failed, which is what the owner asked to see.

**The test buttons.** "Send a test error report" calls `POST /reports/test`, which does what
`ghostd reports test` does and returns what was sent exactly (the project's key shown as a
placeholder) or why nothing was (no key in this build, reports off, a bound held). "Run Ghost's
checks" calls `POST /developer/checks`, which runs this `ghostd` as `ghostd check --json` with
Ghost's environment less every password (`child_env`), a two-minute bound, and gives its lines
redacted, problems first, then warnings, then the rest. It connects each app once, as the
command does. Each row keeps "Copy the command".

All four routes (`PUT /developer/meter`, `POST /developer/checks`, `POST /reports/test`, and the
events) are the control API's: the phone's routes refuse them (`the_phone_has_no_way_to_debug_mode_the_switches_or_a_report`),
the outside listener forwards neither event (`the_live_meter_and_the_timing_overlay_never_reach_an_outside_tool`),
and no tool names them (`no_tool_reaches_debug_mode_or_the_report`).
