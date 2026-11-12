# Two concepts past the direction, November 2026

The owner chose the briefing at the desk and the room during voice
([overhaul-2026-11.md](overhaul-2026-11.md)) and asked for two more samples that are "out of
this world", for review and not to build. The brief: Ghost should feel like talking to and
wielding the power of a supercomputer. Chat bubbles, sidebars of threads, inbox lists and
dashboards of cards are what feels boring.

Both concepts are moving, clickable prototypes in `apps/desktop/src/design/concepts/`. They
are separate from every real screen and read nothing from the daemon:

- `http://localhost:1430/?mock=busy#/concepts/instrument`
- `http://localhost:1430/?mock=busy#/concepts/signal`

(Use whatever port `make dev` or `pnpm --filter @ghost/desktop dev` gives you.) Each opens on
its idle state and plays a scripted sequence of about a minute. You speak; Ghost works across
five apps and answers. A scheduled run finishes in the background. A change waits for your
yes, and an agent asks a question with three choices. Then the screen goes back through the
day and returns to now. Everything stays live while it plays. The switcher (top right, bottom
right on the signal) has Pause, Replay sequence and the keys. URL options: `&at=<seconds>`
starts the script at that point (`&at=30` is the decision), `&motion=reduced` forces the
reduced-motion variant, and `&theme=light` applies to the signal.

How they are built matters to the idea, so it comes first. Both draw one invented day of
work (`day.ts`: six agents, seven apps, 123 calls, 14 runs, four sessions, four questions and changes),
and the screen is a function of the time. `stateAt(t)` returns what was happening at second
`t`: who was speaking and how loud, which calls were in flight, which runs were going, what
was open. So moving back through the day is the same drawing at an earlier `t`, a real
replay rather than a separate history screen. The day's sessions, calls and items are the
same objects Ghost already has (the transcript, `activity`, the audit, the waiting list), so
the replay is the one idea here that the data could support in full today.

## 1. The instrument

**The idea:** Ghost as a machine you play. One dark field, with Wisp as its core, and every
piece of work shown as light in proportion to what it is.

What is on the field:

- **The core** is Wisp at the centre, coloured by the phase colours the app already uses.
  When you speak, your voice comes up from the bottom edge (where you sit) as a stream of
  green motes into the core, as dense as you are loud. While it thinks, violet arcs turn
  around the core, one per call in flight. When it speaks, rings leave the core: each ring
  is the voice level a moment earlier, so the rings carry the shape of the sentence
  outwards. The words appear under the core, said words bright and the rest dim.
- **Apps sit on the outer ring.** Each call is a spark going out along a curve and light
  coming back. The size of the returning light is the size of the result (√bytes). The
  app's node lights up with use, and the tool, item count and size appear beside it for a
  few seconds. A job that reads 37 things in 18 seconds fills the field. One lookup is one
  spark.
- **Agents are bodies in orbit.** Their positions are a function of time, so an agent's
  trail is literally where it was when it did each piece of work: one dot per call, sized
  by what came back, fading over two hours. A body in a run glows, pulses and says what it
  is doing ("thursdays at 2:50 · working · 23 calls"). When a run finishes, a ring goes out
  from the body, a mote travels to the core (the waiting list), and the answer appears
  beside the body for a few seconds.
- **What needs you rises to the front.** It comes out of whoever asked, on a dotted thread,
  and waits on the front row: a decision in amber (amber still means only "waiting for your
  okay"), a question in the asking agent's colour. The words and the choices are real
  buttons. Y or N answers a decision, and 1 to 3 picks a choice.
- **Time is the gesture.** Drag across the field (or scroll, or use the arrow keys, with
  Shift for an hour) and the whole field goes back: the bodies swing back along their
  orbits, old calls fly again, the conversations of the morning are said again, and what
  was open then is in front again with its buttons off. The clock grows to 88px so you
  always know you are in the past. Let go near now and it snaps back. The axis at the
  bottom shows the day: work per minute, your sessions in green, and the times something
  needed you.

**What it makes the operator feel:** that something large is working for them and they can
see all of it at once. The proportion is the point. In most agent tools a
fifty-call research job and a single lookup look the same (one row, one spinner), and in
the instrument they look as different as they are. The orbit trails give a sense of who
has been busy without a single number. Pulling time back is physical, more like running
footage back than opening a history page. The nearest familiar things are voice orbs and node graphs, which are either one blob with no
information or a diagram with no life. (Mobbin was not searched for this pass. These are
judgements, not a survey.)

**What it is bad at, bluntly:**

- **Scanning.** Twenty open items cannot fit on the front row. Three is comfortable and
  five is a crowd. The field has no list, and it should never grow one.
- **Reading detail.** Tool names and sizes show for four seconds and then fade. To read
  what a call returned you need another surface.
- **Accessibility.** The field is one `role="img"` canvas. Everything you must act on is
  DOM (the front row's words and buttons, the caption, the time), but what the field says
  about the work (which agent, how much, where) is visual only. A screen reader user gets
  a much poorer Ghost. Colour carries agent identity.
- **A 9-to-5 at the desk.** It is a lot of motion to keep in the corner of your eye for
  eight hours. Reduced motion stills it (no flying light, no orbit drift, calls shown as
  still lines), but then most of the idea is gone.
- **Light theme: no.** The field is made of light on dark: glows, additive trails, faint
  orbits. On a white ground the glows turn into grey smudges and the trails disappear. It
  is dark only, and the `theme` option is ignored on purpose.
- **Small windows** work down to about 720×520: labels shrink and the front row stacks.
  Below that the field becomes decoration.

**What could be folded into the direction now, without waiting:**

1. **Proportion in the work row.** The conversation screen's "Used GitHub · 2 calls · 1.4 s"
   could carry a small bar per call sized by what came back, so a big read looks big. The
   data is in `activity` already.
2. **The stage's thinking sign as calls in flight:** one arc per open call around the
   presence, instead of a generic spinner.
3. **Ghost's voice as rings** in room view: rings are a 2D drawing from the level signal
   Ghost already has (`voiceLevel`), and they read from across a room better than the
   ribbon does.
4. **A run finishing as a moment.** In the briefing's "what happened" rail, a run that just
   ended could pulse once and name its size ("Research finished · 37 calls · 2.05 MB").

**What it would take to build for real:** a canvas (or WebGL, when the field passes about
2,000 moving things) view on the conversation screen, fed by the session's live
`activity`, the waiting list, and runs from `ghostd::schedules`. The replay needs one new
store query: activity by time across sessions and runs, which today is live only (the
overhaul's own "still to do"). The engine already reads time as its input. A sized result
needs each call's size (bytes and items, never content) carried from `ghost_core::apps`
into `activity`. That is a few numbers per call, inside ADR 0046's bounds and redaction,
and it must stay numbers only. Cost to draw: about 0.6 ms of script per frame at 1440×900
on this workstation with GPU raster, so 60 fps with room to spare. A rough guess: two to
three weeks for the field itself and one for the replay query.

## 2. The signal

**The idea:** no objects at all, only language and time. The screen is one line of very
large type, said by Ghost or by you, with the machine's work running as dense telemetry
beside it.

What is on the screen:

- **The line.** What is being said now, at up to 96px. Ghost's words are set heavy and
  tight and brighten word by word as they are said. Your words are set light and grey and
  appear as you say them, with a green cursor. Run answers, changes Ghost made, questions
  and your answers are lines too. Each has a time and who said it in the margin ("Research
  · thursdays at 2:50", "Ghost · on your yes", "You answered, aloud").
- **The river.** The lines before the current one get smaller and fainter as they go up.
  Scroll, drag or press ↑ and ↓ to move one line at a time through the whole day. Shift
  moves an hour. Only what had happened by then is shown, so the past reads as it was.
- **The signal.** One hairline under the line, the last fourteen seconds, moving right to
  left. Your voice rises above the line and Ghost's is mirrored across it. Thinking is a
  dotted violet run. Every call is a spike below, as tall as what it brought back, in the
  colour of the agent that made it. A research run looks like a fence of yellow spikes.
- **The telemetry margin** is a fine mono column with today's numbers up to the moment on
  screen (reads, changes, bytes in, runs, replies, the last minute), each agent's state
  with a bar of calls in flight, and the last 26 calls: time, agent, `app.tool`, items,
  size and milliseconds. An open call is highlighted and a change is underlined.
- **Deciding is one word.** Under the signal, each open item is its question in a sentence
  and its choices as huge words: **YES  NO**, **BUSINESS  PERSONAL  NOT NOW**. Press one,
  or press Y, N, 1, 2 or 3. In the past the words are greyed and "open then" stands beside
  them.
- **The seismograph** on the left is the whole day, top to bottom, as one trace of work
  per minute. Your sessions are a green bar and the times something needed you are ticks.
  Drag along it to jump.

**What it makes the operator feel:** like standing at the console of something enormous
that speaks to them. The scale of the type says this is the one thing that matters now,
and the density of the margin says how much is going on underneath. It is the most
voice-first of anything Ghost has had, because the screen is literally the conversation,
and it is legible from across a room without a second "room view". It also respects the
operator: no chrome, no cards, nothing to manage, just what was said and what was done.

**What it is bad at, bluntly:**

- **Scanning twenty items.** The answer zone stacks open items under the signal. Two fit
  at full size, and four push the line off the top. A real day with twelve open things
  needs the briefing's Decide, Answer and Know, or a "4 more" line that opens a list.
- **Anything spatial or structural.** Who works on what, schedules, apps and settings have
  no place here, and they should not get one.
- **Telemetry is noise to most people.** It thrills for a week. After that it is either
  ignored or a reason to stare at the screen instead of doing the work. It needs an off
  switch, and that should probably be the default.
- **Accessibility is good.** It is text. Lines are real paragraphs, announced once each
  whole (the word-by-word split is `aria-hidden`), the choices are buttons with
  `aria-keyshortcuts`, and the seismograph and signal are decoration. Contrast follows the
  tokens. The weakest point is the faded upper lines (opacity 0.45 to 0.8), which may fall
  below AA and are meant as ambient context. Anything you must act on is at full contrast.
- **A 9-to-5 at the desk.** Better than the instrument. When nothing is happening the
  screen is still: one line, a flat hairline, the margin unchanged.
- **Light theme: yes.** Ink on paper (`#efede8`) is arguably better than dark: the warn
  amber, the agent hues and the speaking blue all hold. Screenshots are below.
- **Small windows.** At 720×520 the margin folds under the river and the type clamps down
  to 28 to 34px. It still reads.

**What could be folded into the direction now, without waiting:**

1. **Room view is the signal already, half built.** Give it the river (the last three
   lines shrinking above the said line) and the hairline signal with call spikes instead
   of the plain waveform. Both use data the screen has today (turns, `activity`, the voice
   level).
2. **One-word answers.** In room view, an open question's choices as large words the
   operator can read across the room and say. The voice path already accepts the choice's
   words (`questions::match_choice`).
3. **The seismograph as the briefing's day rail.** Next to "what happened", a thin trace of
   work per minute with your sessions marked. It gives the rail a shape without adding
   rows.
4. **The margin's call line format** (`52:10 ghos github.list_releases 3 2.7 kB 412`) for the
   work row's expanded view.

**What it would take to build for real:** mostly typography and the same data the
conversation screen uses. The line and river come from turns and `activity`, the answers
from questions, and runs and changes from the waiting list and the audit. Telemetry needs
call sizes and timings carried into `activity` (as for the instrument). Moving back
through the whole day needs the same activity-by-time store query. The rest is the
existing transcript component restyled, plus two small canvases. A rough guess: one to
two weeks for room view as the signal, and a week more for the day river.

## Recommendation

**Fold the signal into room view now, keep the instrument as the "show me" moment, and build
neither as the app.**

The direction the owner picked is right for living with Ghost all day. The briefing is for
scanning and the room is for talking, and neither concept beats the briefing at scanning.
But the room as built is the weaker half, and the signal is the room taken to its
conclusion. Room view should become the signal: the line at its size, the river above it,
the hairline with spikes, and one-word answers. That fixes room view's thinness (one
reply, a waveform) without adding chrome, and it is mostly CSS over data the screen
already has.

The instrument is the most "supercomputer" thing here and the most unlike other products,
but it is a place to visit, not a place to work. Its best ideas carry without the field:
light in proportion to the work, calls in flight as the thinking sign, a finished run as a
moment, and Ghost's voice as rings. Fold those into the stage and the work row now. Keep
the field itself on file as a full-screen view for when something big is running (a
"watch it work" view, opened by voice) once activity is stored per session. That store
query (activity by time, across sessions and runs) is the one piece of real engineering
both concepts need, and it is worth doing early: it also makes the briefing's "what
happened" honest about runs.

## Screenshots and recordings

Taken from the prototypes at 1440×900 (dark unless named), 1280×800, and 720×520 for the
small window. The paths are in the PR's report and stay out of the repo, as for the
overhaul.
