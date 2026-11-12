# The interface overhaul, November 2026

The owner's bar: every feature at 9/10 is not enough; how Ghost is used has to be 9/10 too,
and find its own ways to present itself. Ghost is voice first: someone speaks to it across a
room and glances at the screen. It is not a chat app with a microphone bolted on.

**Decided, 2026-11-12: the combined direction.** The owner, in their words: “Let's go with the
combine impl.” The briefing when the operator is at the keyboard, the room whenever a voice
session is live or they have stepped away. What they said about taste: the room “doesn't feel and
look like everything else, which is exactly what I don't like about the console”; “I do admit
that I like the timeline from the briefing”; and “a lot of these systems are boring and they
don't really feel like communicating with and wielding the power of a super computer and that's
what I want Ghost to feel like.” The console is rejected. Built as the rail, Today with its
timeline and the two postures (docs/design-system.md, The two postures, Today, The timeline);
the `#/directions/...` mock pages are removed. Section 4 records what was built and what is left.

This document has three parts: what was done to the conversation screen (shipped in the
same PR), an audit of every other screen, and three directions for the rest of the app as
clickable pages, with a recommendation and an order to build in. Nothing outside the
conversation screen is restyled until the owner picks a direction.

Open everything with `make dev` (or `pnpm --filter @ghost/desktop dev --port 1430`) and the
URLs below. Screenshots from this pass were taken with `.scratch/design/tools/shot.mjs` at
1440x900 and live outside the repo (paths in the PR's report).

## 1. The conversation screen (done)

### Why it was a 4

Seen in every state with the mock scenarios before the change:

- **It was a chat.** The operator's words in grey bubbles on the right, Ghost's unboxed on the
  left: the shape of every text chat app, for a product people speak to.
- **Nothing showed the voice.** Ghost speaking was a 92px pill in the composer's corner; the
  reply in the transcript looked the same whether it had been said, was being said, or had
  been cut off halfway. The operator speaking showed nothing at all.
- **Nothing showed the work.** Between "What's waiting on me?" and the answer, Ghost called
  two tools; the screen showed "Thinking…" and then text. Handovers from the fast model,
  changes Ghost made at `quiet`, a failed turn, a question an agent asked, hold: none of them
  had a place.
- **Time was either everywhere or nowhere.** One time per name, nothing for a nine-minute
  pause, the same "Session started" over and over in the history with no end and no length.
- **Unreadable from across the room**, where the operator is when they talk: 14px text in a
  680px column, the state in a sidebar card.

### What it is now

`?mock=thread`, `you`, `cut`, `hold`, `ending`, `turn-error`, `speaking`, `thinking`,
`pending`, `pieces`, `passive` (history), `empty`, light and dark, and 1024 wide. The contract
is in docs/design-system.md, “The conversation screen”.

- **A record on a rail**, one mark per kind of moment, time where a conversation has it (the
  first row, after a pause, “9 min later” in the gutter with the rail dotted across it,
  “Session ended · 4 min”), Ghost's words brightest.
- **Work as one row** between turns: “Used GitHub · 2 calls · 1.4 s”, opened to each call.
- **The reply being said** moves word by word with Ghost's voice; a cut leaves a mark where
  it came and the unsaid part dim.
- **The stage** on the composer: Ghost speaking with a waveform from its own voice, the
  operator speaking as a breathing ring (never words: #73), the apps in use, hold, goodbye.
- **Room view**: the same moment at 44px from across the room, on by itself after 20
  seconds with nobody at the keyboard in a spoken session.
- **Questions answered in place**, **a reply heard again** in its agent's voice outside a
  session.
- The daemon's side is ADR 0055: Ghost's voice level (already there) and the session's
  activity by name, never words, off the phone.

### What the category taught (Mobbin, and the open-source agents read, not run)

- Meta AI's voice mode and Brink's transcript brighten what has been said and dim the rest:
  the said-so-far estimate is that idea, with the voice level as its clock.
- YouTube's and Browserbase's transcripts put time in a left gutter: the rail's time column.
- Tana's “Thought for 52 seconds ›”, Lindy's and Zapier's collapsed action rows, Goose's
  `ToolCallWithResponse` (tool name, app, a status dot, expandable output) and OpenClaw's
  coalesced “activity runs”: the work row, one per run of calls, closed by default.
- Gumloop and Goose keep a question or an approval inline where it was asked, with its
  choices: the question row.
- Google's and Meta's voice screens give the state one large word and one large sign:
  the stage, and room view.

### Still to do on this screen

- What came back from a read: needs a bounded, redacted preview from the model stage
  (`ghost_voice::llm`, another agent's area today), within ADR 0046's limits.
- An exact word clock: the TTS stage stamping audio with its text, if the estimate shows
  wrong in use.
- A cut, a call or an error are live only; a reload shows turns, the audit's changes and the
  questions. Storing activity per session would need the store's schema.

## 2. The audit: every other screen against the same bar

Scores are the same scale the owner gave the thread (it was a 4).

| Screen | Score | What holds it back |
|---|---|---|
| Sidebar (every screen) | 5 | Thirteen agents as nav rows push everything else down and scroll; “Agents” is both a nav item and a list; the status card and the agent row say the same thing twice. |
| What needs you | 6 | The right model (one list, an answer goes back) in a dense ledger: each row has a state badge (New, Sent not heard, Heard), Got it, a bell, sometimes Confirm/Cancel; five colours of meaning; no grouping by what you do about it; the page opens with a 4-line paragraph. |
| Agents | 5 | A table of personas: name, persona text truncated, job, time. Says nothing about what an agent has done or will do (its schedules, its last answer, what it asked); “New agent” is the primary action on a page you mostly read. |
| Agent detail | 5 | Five tabs of settings around a transcript in the old bubble style; should share the conversation screen's record. |
| Watching | 5 | Two amber notices stacked before the content; cron strings in mono as the primary description (“On 0 9 * * 1-5”); schedules, watches and webhooks as one long page. |
| Apps | 6 | Blank white squares where logos should be (reads as broken); a raw command error in red as the row's subtitle; “Apps” is both in the main nav and under Settings. |
| Settings | 6 | Ten pages, each fine; the nav duplicates Apps and Audit log; General opens with build detail before anything you would change. |
| Onboarding (Welcome, prototype) | 4 | Its promise is out of date: “It asks before it changes anything… waits for your spoken yes” is ADR 0007, not ADR 0034 (everything runs unless your permissions say otherwise). “5 of 6 done” over eight steps. |
| Tray | 7 | Clear and small; “Mute microphone” is being replaced by today's not-listening control. |
| Phone (Android) | 6 | Calm and on brand; “Recent decisions” is the gate's wording, not the operator's; Home has nothing that needs you, only what happened. |
| Every page | — | Each opens with a 3-4 line explanatory paragraph under the title: documentation where the content should be. |

## 3. Three directions for the whole app

Each is a real page over the mock daemon's data, Home and Agents, with a switcher to compare:

- `http://localhost:1430/?mock=busy#/directions/room/home` and `…/room/agents`
- `http://localhost:1430/?mock=busy#/directions/briefing/home` and `…/briefing/agents`
- `http://localhost:1430/?mock=busy#/directions/console/home` and `…/console/agents`

### A. The room: the conversation is the app

No sidebar: a top bar (Room, Agents, Watching, Settings). Home is Ghost's presence at 160px
with one word at 44px, and beside it what needs you as large cards in Ghost's words, “Ghost
will bring these up when you talk.” Agents are people you call: a grid of cards with the
agent's glyph, job, when you last spoke, and Call.

*Trades:* the most voice first and the most legible from across a room; the least to scan
when you sit down, and settings and watches are one level further away.

### B. The briefing: a chief of staff's desk

A narrow labelled rail (Today, Agents, Watching, Apps, Settings), no agent list in it. Home
is Today: the date, one sentence (“2 things need you · 8 changes made · 4 to know”), what
needs you grouped by what you do about it (Decide, Answer, Know), each with one action, and
what happened today on a rail beside it. Talking to Ghost is a bar at the bottom with the
presence and the waveform; in a session it opens into the conversation screen. Agents is a
roster: who, what for, when last, Open.

*Trades:* the best at the desk and the closest to the chief-of-staff direction
(docs/direction.md); voice is one bar until a session starts, so the room view matters more.

### C. The console: everything at once

A 64px icon rail with agents as glyphs (a dot on the live one), a 360px queue, and the
conversation always open beside it. Agents is a list and a profile.

*Trades:* nothing is ever more than one click away, and it is the densest; it is the most
like other tools, it is busy, and it is the worst from across a room.

## Recommendation

**B, the briefing, with the conversation screen as its session.** Ghost's job is a chief of
staff's: things come in, Ghost acts on most of them and says so, and the operator decides,
answers or notes the rest. The briefing makes that the first thing seen and groups it by
what the operator does, which the waiting list and the audit can already supply. When a
session starts, the conversation screen (with its stage and room view) takes the page, so
the voice-first moment loses nothing; A's large cards come back as room view's idle state
(what Ghost will bring up), so the best of A is kept. C is the right shape for nothing Ghost
does.

## Order to build it in

1. **The frame**: the labelled rail; agents out of the sidebar (they live on Agents and in
   the conversation); one place for Apps. Every page drops its explanatory paragraph to one
   line or none.
2. **Today** (Home): Decide, Answer, Know from `GET /waiting`, what happened from the audit
   and runs; the talk bar; the conversation screen on wake. What needs you becomes Today.
3. **Onboarding copy** to ADR 0034, whatever the direction: it promises behaviour Ghost no
   longer has.
4. **Agent detail** on the conversation screen's record (one transcript component, two places).
5. **Agents** as a roster with what each did last and has scheduled.
6. **Watching**: plain-words schedules (“Weekdays at 9”), one notice for all problems.
7. **Apps**: logos or a lettered tile, never a blank; errors in words with the detail on open.
8. **The phone**: Home with what needs you first, the same Decide/Answer/Know.

## 4. What was built (the combined direction)

- **The frame (step 1)**: the rail replaces the sidebar: Ghost's presence at the top (the way to
  the conversation, now `#/session`), Today, Agents, Watching, Apps, then Hearing, Settings, the
  room and the microphone. No agent list. Explanatory paragraphs: Today has none.
- **Today (step 2)**: `#/` is the briefing. The day in one sentence with real counts, the
  instruments (working now, tool and model calls, apps in reach, how fast replies start), and the
  timeline as the spine: Now at its head with Decide, Answer, Know and the agents at work (live
  clocks, Stop), then the day hour by hour, every change, run, watch finding and question, with
  clusters, quiet gaps, filters, a day map, the keyboard throughout and 30 days back. What needs
  you is Today now; `#/waiting` redirects.
- **The two postures**: one rule (`postureOf`), the room over the desk after 3 minutes away or 20
  seconds into a spoken session, held with Ctrl+.; on the conversation screen the rail steps out and
  its own room view takes the window. Settings › General sets it.

Where each old screen's functions went: the sidebar's status card is the rail's presence; its
agent rows, New agent and Retired are on Agents; its Prototypes group and the scenario switcher
are the rail's design menu (development only); the version line is the presence's tooltip and
Settings › General. What needs you is Today's Now (every action kept). The session at `#/` is the
conversation screen at `#/session`, unchanged inside.

Still to build, in order: Agents as a roster with what each did last and has scheduled (step 5),
agent detail on the conversation screen's record (step 4), Watching in plain words (step 6), Apps
with lettered tiles (step 7), onboarding's copy to ADR 0034 (step 3), the phone (step 8).
