# Kept wake takes and retraining on this computer, 2026-11-13

ADR 0065 and issue #101: Ghost keeps each wake attempt, and `make wake-train` retrains the
"hey ghost" detector on them. This note covers three things: the keeper running live, what
local retraining costs, and whether retraining on the takes already on disk beats the
shipped detector. It quotes no speech.

## The keeper, live (loopback)

An isolated daemon (every path in a temp folder under `~/.local/share/ghost/`) listened on
a PipeWire null sink's monitor, remapped as a source. Into it went:
- six Kokoro clips in voices the replay rig does not use (three "hey ghost", three
  confusables);
- three stretches of the operator's 2026-10-30 `ghost.mic` recording around weak takes.

Settings were the operator's: `onnx`, threshold 0.6, floor 0.3, check off. Nothing played
in the room.

- 8 takes kept: 7 accepted and 1 refused, each with the array's own stream beside it (the
  real XVF3800, room quiet).
  - Each has its score curve and what Parakeet read.
  - Files are 0600 in a 0700 folder.
  - An accepted take of 12.1 s with the array beside it is 1.16 MB.
- `GET /wake/takes` listed them with no audio; the choice read `keep: true` by default.
- A take decided within a few seconds of the passive listener starting holds only what was
  heard since it started: 2.3 s and a single score, not 12 s. The listener is rebuilt after
  each session, so a wake straight after a hang-up has a short take.
- The shipped weak-hit check woke on two of the three Kokoro confusables ("He goes…"). That
  is the known behaviour of the 0.8 match (issue #13,
  [2026-11-12 wake check](2026-11-12-wake-check.md)), and these takes are the data that
  measures it.

**Not done: playing clips through the room.** The desk has one speaker, the reSpeaker's own
output:
- the XVF3800 cancels it on chip before any of its streams;
- `ghost.mic` cancels the default sink again.

So a clip played through it is removed before the detector hears it, by design. Training
data through the real acoustic path has to come from the operator using Ghost, which is
what the keeper collects.

## Retraining cost on this machine (i5-11600K, no GPU, shared with other jobs)

| step | time |
|---|---|
| features of the labelled streams, 42 streams (about 2.7 h of audio), 8 processes | 194 s, once (cached) |
| one embedding per 80 ms step, one core | 5.8 ms (matches the scorer within 3e-5) |
| 480 synthetic Kokoro clips, 10 processes | about 30 min, once (cached) |
| one fit of the head (about 100,000 windows, 30 epochs, one BLAS thread) | 5 to 10 min |
| the whole run: 3 cross-check folds and the candidate, scoring every set | 37 min |

Training locally is practical. Compute is not the limit; the data is.

## Does it beat the shipped detector? Not yet.

Run `run1` (`.scratch`, not installed):
- **Training data:** the 21 takes of 2026-10-30, three folds by recording, 24 augmented
  copies each; Kokoro voices; music.
- **Head:** beside the shipped classifier, combining its score.
- **Scoring:** under the daemon's rule, where the first frame at the floor decides (weak:
  Parakeet checks; at the threshold: wakes at once).

| | shipped 0.60 / 0.30 | candidate, its own point 0.82 / 0.41 |
|---|---|---|
| operator takes, `ghost.mic` (cross-checked), woke at once + checked / 21 | 3 + 7 | 13 + 7 |
| operator takes, array (cross-checked) | 8 + 8 | 20 + 1 |
| held out recording only (7 takes), `ghost.mic` / array | 1 + 3, 4 + 3 | 2 + 4, 7 + 0 |
| false wakes over every set | 0 | 68 |
| Parakeet asked about something that was not a take | 10 | 114 |

It hears far more, especially on the array, but it pays in false wakes (issue #13): 30 on
the room recordings of the Kokoro rig alone.

A sweep of its threshold and floor under the same rule found no point that keeps the shipped
detector's record:
- at 0.99 / 0.30 it still makes 5 false wakes, asks Parakeet 207 times, and hears 0 of the
  held-out `ghost.mic` takes at once.

`ghostd detector use run1` refuses it on its card ("wakes more often than the shipped
detector … 2 false wakes, against 0") and leaves the setting alone.

**Why:**
- 21 takes from 7 recordings are too few for the head to learn the operator's voice without
  learning the recordings' room as well.
- The negatives it saw (synthetic confusables, music, about 7 minutes of the operator's
  speech) do not cover what it then fires on.

**The fix to the command:**
- The first run chose its threshold on a detector-alone curve.
- The command now chooses it under the daemon's rule, against the shipped detector's false
  wakes and Parakeet checks on the training negatives.
- When no point keeps both, it writes nothing installable (`run2`).

## A rented GPU

Not used. A GPU would shorten the fit, which already takes minutes, and would allow a full
`livekit-wakeword` retrain from thousands of synthetic voices, which is what the shipped
detector already is. Neither adds the operator's voice. The limit is the number of their
takes, which the keeper now collects. The owner allowed RunPod for this (2026-11-12); it is
worth reconsidering when a few hundred labelled takes exist and the local head still trades
false wakes.

## What to measure next

After a day or two of kept takes, labelled:

1. `make wake-train`, then read `report.md`. Every false-wake row should be no higher after
   than before; then compare the cross-check's "woke at once" on `ghost.mic`.
2. `make wake-detectors` refuses any candidate the card shows waking more often.
