# 0044: The model hears the whole turn, read once

- **Status:** accepted
- **Date:** 2026-11-06
- **Amends:** ADR 0018 (a turn of two segments or more costs one more read of the model at
  the close; measured below)

The speech-in stage reads each VAD segment on its own and the user aggregator joins the
pieces with a space. A sentence cut at a pause ("Remind me tomorrow morning ... to review
the Phonon notes") reaches Parakeet as two sentences, each without the other's context,
and it loses words at the cut: on the operator's recorded session of 2026-11-06
([#60](https://github.com/heka-vault/ghost/issues/60)), 24 of 49 spoken takes reached the
stage in pieces, and the hear harness's `turn` read, the same audio from the first
segment's pre-roll to the last segment's end read once, scored **4.2%** word error rate
against **8.4%** for the pieces, with no empty reads. That is item 1 of the STT review
([#52](https://github.com/heka-vault/ghost/issues/52)). It only became safe to build once
Parakeet ran at full precision (ADR 0043): the int8 model read nothing for 2 of those 49
turns.

## Decision

The user aggregator keeps the turn's audio on a tape (`ghost_voice::turns::TurnTape`): a
one-second pre-roll ring while no turn is open, then everything from the turn's start,
speech and the pauses between, up to the last VAD stop. When a second or later segment's
transcript arrives, the aggregator reads the tape once with the same model and the same
trailing silence and dither a segment gets, and that one text replaces the turn's pieces;
the segment's own transcript is used only when the whole read says nothing. A second or
later segment the model read as nothing, with speech's level in it (`HeardNothing`, at
least `HEARD_NOTHING_MIN_DBFS`, −45 dBFS), is read whole the same way (amended
2026-11-12: a sentence split inside a word had its second half, 3.6 s at −21 dBFS, read as
nothing, and was answered from its first half); if the whole read hears nothing too, the
turn closes on the pieces it has and the log says so. A
single-segment turn is read once already and costs nothing more. Words that stood before
the tape's first segment (a reopened turn's, or a segment whose audio went by before the
turn opened) stay in front. Over `TAPE_MAX_SECS` (30 s) the tape stops and the pieces
stand. `GHOST_TURN_REREAD=false` turns it off; the hear harness's `turn` read is the
measure of it against `stage`.

The stages before the aggregator (the echo filter, the hold gate, the interceptors, the
confirmation path, the transcript tap) still see each segment as it comes, so nothing
waits for the whole read but the model's turn and the speculative answer, which now
answers the whole text.

## Consequences

- The model, the speculation and the transcript store get the whole turn. Since
  2026-11-12 the store keeps a spoken turn once, at its close, with the words the model
  was given (`ghost_voice::tap::TurnRecord`, from the aggregator, or from an interceptor
  that took the turn), and the gate's request (ADR 0017) is that turn too; a turn
  reopened before its answer was heard takes its record back and is kept once when it
  closes again (`ConversationRuntime::hold_user`). Before, every segment was a user turn
  of its own, a wrong first read included, and they came back as consecutive user
  messages in the next session's context. The pieces still go out as they come to what
  shows them live (`UserTap`, the desktop's captions).
- One more model read per turn of two segments or more, of the whole turn's length, at
  the close: about 30 ms per second of audio with fp32. The numbers are below.
- The aggregator holds up to 30 s of the session's audio in memory while a turn is open;
  the Smart Turn analyzer already held 8 s. Nothing of it is written anywhere, and the
  passive path has no aggregator.

## Measured

`make perf RUNS=2 ARGS=--no-jev`, the same afternoon as ADR 0043's numbers, the fp32
model both times (the `lookup_pause` scenario is the one with a pause mid-sentence):

| | pieces | whole turn |
|---|---|---|
| `stt` stage p50 / p95 | 0.17 / 0.25 s | 0.18 / 0.24 s |
| `hear` p50 / p95 (end of speech → the model has the turn) | 1.45 / 1.97 s | 1.46 / 1.96 s |
| reply p50 / p95 | 2.05 / 2.59 s | 2.09 / 2.59 s |
| `lookup_pause` reply p50 | 2.14 s (int8 day: 2.96 s) | 2.14 s |

The whole read of the pause scenario's 4.5 s turn took 0.24 s and finished inside the
grace, so nothing after the words waited for it: `hear` is unchanged. The hear harness's
`turn` column (ADR 0043's table) is what the model now gets for a split turn.

## Alternatives considered

- **A longer VAD stop, so a pause doesn't end the segment.** At 1.0 s the stage splits 7
  takes instead of 24, but every segment then ends in a second of quiet, and the error
  rate rose to 8.8% with fp32 (12.3% with int8); and every turn would wait that second.
- **Re-read in the speech stage and emit the whole turn as the segment's transcript.**
  Every stage below sees finals, and the transcript tap stores each one: the whole turn
  would be stored again beside its pieces, and the echo filter and the interceptors would
  judge a growing text each time. The aggregator is the one stage that owns the turn.
- **Read at the close instead of when the segment's transcript arrives.** The close is
  the grace after Smart Turn's verdict; reading then would delay the answer by the read,
  and the speculative answer would have answered the pieces. Reading when the transcript
  arrives puts the whole text in the speculation's hands with most of the grace to spare.
