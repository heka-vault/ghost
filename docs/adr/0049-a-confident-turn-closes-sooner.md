# 0049: A confident turn closes sooner, and the voice is ready when it does

- **Status:** proposed
- **Date:** 2026-11-12
- **Amends:** ADR 0018 (the turn grace is no longer one number; a reply's first clause is
  rendered before the reply is released), ADR 0044 (a reopened turn is still read whole)
- **Builds on:** the reopened turn of 2026-10-29 (`ghost_voice::turns::TurnControl`)

Every turn waited the same 1.2 s after Smart Turn heard an end, in case the operator went
on. The wait was right for a pause mid-sentence and wrong for "What time is it?", and with
the 0.2 s of silence the VAD needs it put a floor of 1.4 s under every reply before the
model or the voice had done anything. The leaders close in 0.2 to 0.55 s when their turn
model is sure, and take the turn back when the user goes on. Ghost already had the taking
back: since 2026-10-29 a turn the operator goes on from before any of its answer was heard
or done is reopened whole. That is what makes a shorter wait safe to try.

## Decision

**The wait depends on how sure the end is** (`ghost_voice::turns::TurnTiming`). Two signs
must agree for the short one: Smart Turn's probability that the turn is done is at least
`GHOST_TURN_CONFIDENT_PROB` (0.9), and the words end a sentence (a full stop, a question
mark or an exclamation mark Parakeet wrote), don't stop mid-thought (`sounds_unfinished`)
and don't end on a number. Then the turn closes after `GHOST_TURN_CONFIDENT_GRACE_SECS`
(0.35 s). Everything else waits what it waited:

| The end | Wait from the VAD's stop |
|---|---|
| Smart Turn sure (0.9 or more), the words end a sentence | 0.35 s |
| Smart Turn says done, less sure, or no stop was written | 1.2 s (`GHOST_TURN_GRACE_SECS`) |
| ... and the words end on a number | 1.7 s (the grace and `GHOST_TURN_NUMBER_SECS`) |
| The words stop mid-thought, whatever Smart Turn says | 3 s (`GHOST_TURN_UNFINISHED_SECS`) |
| Smart Turn says not done, the words end a sentence | 1.6 s (`GHOST_TURN_STOP_SECS`) |
| Smart Turn says not done | 3 s |

The words reach the turn a moment after Smart Turn's verdict, so the short grace is taken
when they arrive, never on the verdict alone and never on the words of the segment before
("What's waiting for triage?" then a pause, then more: the second segment's verdict must
not close on the first segment's question mark). A short grace at or over the grace, or a
probability over 1, is the one grace for every turn, as before.

**What the turn does waits as long as it did.** The short grace is for speaking sooner.
A tool call of a turn that closed on it does not run until the grace itself would have
run out, 1.2 s after the end of the speech (`TurnControl::settle_at`): a model quick
enough to have its call ready in half a second would otherwise read, or change,
something for half a sentence while the operator was only pausing. With the main model
the call is never ready that soon, so the wait costs nothing; with a quick one it costs
what is left of the 1.2 s. The acknowledgment before a hand-over (ADR 0050) waits the
same way.

**Going on reopens the turn, as it did, and a little further.** The operator going on
before any of the answer was heard or done still gets the turn back whole. Three things
are new:

- The turn's tape runs on after the close, so a reopened turn is read once from its first
  word to its last, pause included (ADR 0044). Before, the second half was read alone.
- When the operator's words cut in on an answer whose first clause had only just begun
  (their speech began within `GHOST_TURN_REOPEN_STARTED_SECS`, 0.15 s, of Ghost's first
  sample) and nothing was done for the turn, it is the same turn too: the clause stops,
  counts as not heard, and the half answer leaves the context. They were going on, not
  answering Ghost. Later than that, the words are a barge-in on an answer that stands, as
  before. Words decide this, as they decide any barge-in: the VAD alone still never stops
  Ghost. `hear.reopened` in `latency.jsonl` says `unheard` or `started` on the turn that
  went on, so how often each happens is counted.
- A reply is recorded as said when Ghost's voice starts on it, not when the model finished
  writing it (`ghost_voice::tap::AssistantTap`). A reopened turn's half answer was never
  said, and no longer lands in the transcript or the agent's history.

**The reply's first clause is rendered while the reply is held**
(`ghost_voice::tts::Prerender`). The model's stage hands the voice each reply's first clause
the moment the model has written it: during the grace for a held answer, and while the
narration cut holds a reply to see whether it calls a tool. When the reply is released the
TTS stage plays that audio instead of rendering the clause then. Rendering is not
speaking: nothing reaches the speaker, the echo filter or the transcript until the reply
is the answer. A reply that is withdrawn, or that calls a tool after its opening words,
drops the audio: about a third of a second of Kokoro on a blocking thread, counted as
`prerender.dropped`. With a paid voice a dropped clause would be paid characters, so
`GHOST_TTS_PRERENDER` is on for Kokoro and off for the cloud voice unless set. Rendering
ahead through ElevenLabs is not built: its audio streams back over the socket as it is
made, and holding it needs its own buffer per context.

## Measured

The safety of the shorter wait, on the operator's recorded takes of 2026-11-06 (49 spoken
takes, each one turn as he meant it, 24 of them reaching the stage in pieces with 39
pauses mid-turn between them; `make perf-turns TAKES=latest`, the real VAD, Parakeet and
Smart Turn, and the same rule the stop strategy runs):

| | one grace, 1.2 s | short grace 0.35 s at 0.9 |
|---|---|---|
| Pauses mid-turn the wait was shorter than | 1 of 39 | 3 of 39 |
| ... still closed 1.2 s after the VAD's stop | 1 | 1 |
| Wait at a turn's real end, mean | 1.37 s | 0.85 s |
| Wait at a turn's real end, p50 | 1.20 s | 0.35 s |
| Real ends that got the short grace | 0 of 49 | 30 of 49 |

Two more pauses close early, of 0.80 s and 0.90 s, in "What's on my plate ... for this
afternoon?" and "Remind me tomorrow morning ... to review the Phonon notes." Both first
halves read and sound like ends, and both pauses are over before any voice could start,
so both turns are reopened whole. The third (1.32 s) was already closed early by the
1.2 s grace. Most real pauses mid-thought never reach the short grace: Smart Turn called
20 of the 39 not done, and of the 19 it called done the words stopped mid-thought at 3,
ended on a number at 7 and carried no stop at 3. Six were left, four of them at 0.9 or
surer, one of those shorter than the short grace itself. Kokoro's rendered takes are
harsher, because it speaks each half of a paused line as a finished sentence: 12 of 65
pauses close early, none still closed at 1.2 s.

What it buys in `make perf` is in
[the benchmark](../benchmarks/perf/2026-11-12-voice-speed.md): with the scripted model
taking a small model's time, a plain reply's p50 goes from 2.05 s to about 1.5 s; with it
taking the main model's 1.2 s to a first token, almost nothing, because the model and not
the grace was already the wait.

## Considered options

- **A shorter grace for every turn.** 0.35 s for every end Smart Turn calls done would
  close 9 of the 39 real pauses early instead of 3. The grace is short only where two
  independent signs agree.
- **Smart Turn's probability alone.** It was 0.97 and 0.99 at two pauses inside "I
  noticed that when I pause ... in the middle of a sentence ... you cut me off", and 0.98
  at one inside "Close issue one forty two": the voice fell, the sentence had not ended.
  The words caught all three (`sounds_unfinished`).
- **Stopping Ghost on the VAD alone when the operator's speech began before Ghost's
  voice.** It would stop a just-started answer a few hundred milliseconds sooner than
  waiting for words. But the VAD is reset when Ghost starts speaking, so the evidence of
  who began first is gone, and "the VAD alone never stops Ghost" is the rule that ended
  Ghost hearing itself (2026-10-29). Not worth weakening for this.
- **Rendering every clause ahead, not only the first.** The first is the one the operator
  waits for; the rest are rendered while it plays, as before.

## Consequences

- A turn closed too soon costs a model call thrown away (`hear.speculation`: `withdrawn`),
  one rendered clause dropped, and nothing the operator hears. It costs more often than
  before: on the recorded takes, 3 pauses in 39 against 1.
- When the turn closes sooner than the model answers, the wait is the model's alone, in
  silence. The thinking sound is off (2026-10-30) and nothing fills it yet.
- `hear.close` in `latency.jsonl` says how each turn's close was timed (`confident`,
  `grace`, `number`, `unfinished`, `sentence`, `incomplete`), and `prerender` counts
  clauses rendered, played and dropped.
- The numbers above are from recorded takes and a scripted model. The thresholds were
  chosen on one session of one voice at one desk; the voice rubric (parts B, C and D) on
  the real microphone with the real model is what confirms them.
