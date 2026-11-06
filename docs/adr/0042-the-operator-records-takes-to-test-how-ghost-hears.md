# 0042: The operator records takes to test how Ghost hears, in Ghost's own app

- **Status:** accepted
- **Date:** 2026-11-06
- **Amends:** ADR 0013 (the control API carries levels and what Ghost heard of a take the operator
  recorded on purpose, never of passive audio), ADR 0040 (meeting notes keep no audio; takes are
  the one audio Ghost keeps, and only because the operator recorded them)

Whether a change to the speech-in path ([#60](https://github.com/heka-vault/ghost/issues/60),
[#52](https://github.com/heka-vault/ghost/issues/52)) helps is measured on the operator's own
voice through the operator's own mic and room: the hear harness (`make perf-hear`,
`docs/benchmarks/hear-harness.md`) scores a set of takes. The first recorder was a terminal
script that gave each line a fixed time worked out from its word count, and cut the operator
off mid-sentence when they spoke slower than the estimate. A take that lost its last words
scores as Ghost's error, so the test set was wrong where it mattered.

## Decision

**The Hearing test is a screen in Ghost's desktop app, and the daemon records.** The daemon
already holds the microphone and the models. Opening the recorder hands the mic from the
passive loop to the recorder, the way a phone call does: Ghost is not listening for its name
while it is open, the status says so, and closing the screen (or ten minutes with no take)
gives the mic back. A session in progress refuses it.

**The operator says when a take starts and ends.** Hold to talk: Space or the button down
starts the take, up ends it; 0.5 s before the press and 0.5 s after the release are kept, so a
late press or an early release does not cut a word. Takes the operator cannot reach the key
for (across the room) start after a countdown and end 2 s after the last word the VAD hears.
Silent takes run for the plan's fixed time. No take ends on an estimate of how long the line
takes to say.

**Each take is checked before the next.** Parakeet reads the take at once, and the screen
shows the line against what Ghost heard, word by word, with the level, clipping and whether
speech ran into either end of the take. The operator keeps it or redoes it. What Ghost heard
is shown and never written: the manifest keeps the line the operator was asked to say, the
levels and the speech span, not a transcript.

**What is kept, and where.** Takes, the operator's take list and the score runs live under
`~/.local/share/ghost/hear/` on this machine: the takes as 16 kHz WAV (`ghost.mic`, and the raw
array beneath it), runs as numbers (`results.json`) and their report. Nothing is uploaded,
committed or sent to a model. The operator deletes a session from the screen.

**What the control API carries.** While the recorder is open, a `hear` event with the mic's
level, whether the VAD hears speech and the take's state, about twenty a second; after a take,
its check, including what Ghost heard. Takes are played back through Ghost's own output, so
audio never crosses the API. None of this exists outside the recorder: the passive loop's
audio and transcripts still never reach the API (ADR 0013, ADR 0010).

## Consequences

- The terminal recorder (`scripts/record-hear-takes.py`, `make record-hear`) is retired;
  `make perf-hear` reads the app's sessions, and still the older `.scratch/hear-takes/` ones.
- The score runs in the daemon on its loaded models while the recorder holds the mic, so a run
  never competes with a session for Parakeet.
- A new phase, `recording`, in the status the desktop and the phone read.

## Considered

- **A separate recorder app.** Its own shell and its own copy of the models, and Ghost stopped
  while it records. One app was the operator's ask (ADR 0038).
- **Ending on silence for every take.** A thinking pause (block B) or a soft voice (block E)
  ends the take early unless the silence is long; holding a key is exact and costs a key.
- **Streaming the take's audio to the webview to play it.** A path for microphone audio over
  the API that nothing else needs.
