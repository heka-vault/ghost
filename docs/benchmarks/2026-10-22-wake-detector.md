# Trained "hey ghost" detector: the full model on the replay rig, 2026-10-22

**Model:** `livekit-wakeword` 0.3.0, `conv_attention` medium, 50,000 steps. Trained on:
- 15,000 synthetic "hey ghost" clips from all 904 voices of Piper's LibriTTS checkpoint;
- the library's CMUdict neighbours plus 29 named confusables ("hey guys", "he goes", "hey Gus", "hey coach", "hey goat" …);
- room impulse responses and noise, three augmentation rounds.

The run took about 7 hours at low priority on the i5-11600K (no GPU); training alone was 2 h 40 min.

The model is 0.9 MB. The mel and embedding front ends ship with `livekit-wakeword`. The 32 GB of training data isn't needed to run it.

**Rig:** the [wake review](2026-10-21-wake.md)'s replay. Kokoro clips in 16 voices:
- **digital:** 80 wake clips (48 of them "hey ghost" phrasings) and 120 negatives, half of them confusables;
- **room:** 40 wake (24 "hey ghost") and 60 negatives, played from the desk speakers;
- **mics and channels:** the array's channel 0 (what `ghost.mic` is built on) and the webcam;
- **variants:** quiet, with music, and 13 dB quieter ("far");
- **read speech:** 6.1 h of LibriSpeech, for false wakes.

Only "hey ghost" wakes Ghost in detector mode, so the counts below are of those phrasings. The detector scores the last 2 s every 80 ms, as `ghost_voice.wakeword` does live.

**Not tested:** the operator's own voice. Their takes (`make record-takes`, [docs/wake-recording.md](../wake-recording.md)) are ADR 0025's acceptance test.

## Results

**Detector alone, by threshold.** Each cell: "hey ghost" heard / false wakes on the negatives (out of 120 digital, 60 room).

| threshold | digital | desk, array ch0 | desk, webcam | music, ch0 | far, ch0 | read speech, 6.1 h |
|---|---|---|---|---|---|---|
| 0.3 | 46/48 / 21 | 24/24 / 9 | 24/24 / 20 | 17/24 / 4 | 12/24 / 9 | 2 |
| 0.5 | 46/48 / 17 | 24/24 / 7 | 24/24 / 12 | 13/24 / 4 | 11/24 / 4 | 2 |
| 0.6 | 45/48 / 10 | 24/24 / 6 | 24/24 / 10 | 10/24 / 2 | 11/24 / 3 | 1 |
| **0.7** | 45/48 / 7 | 24/24 / 5 | 23/24 / 6 | 8/24 / 1 | 11/24 / 3 | **0** |
| 0.8 | 40/48 / 3 | 24/24 / 3 | 23/24 / 5 | 5/24 / 0 | 8/24 / 0 | 0 |
| 0.9 | 31/48 / 2 | 19/24 / 1 | 21/24 / 5 | 5/24 / 0 | 5/24 / 0 | 0 |

The false wakes left at 0.7 are confusables written to sound like the phrase: "Hey Gus, how have you been?", "Hey coach, what time is practice?", "Hey, goat cheese or cheddar?", "He goes to the gym every morning."

**Against transcript wake.** "Hey ghost" phrasings heard. The last column is when the chime could play for "Hey Ghost.", relative to the end of the speech, at the desk.

| wake path | desk, ch0 | desk, webcam | music, ch0 | far, ch0 | false wakes | chime |
|---|---|---|---|---|---|---|
| transcript wake (fuzzy 0.9, 0.5 s stop) | 20/24 | 16/24 | 4/24 | 2/24 | 0 | about 0.5 s after |
| detector + check of the phrase (to 0.3 s after the hit) | 15/24 | 11/24 | 5/24 | 1/24 | 0 | 0.10–0.15 s after |
| detector + check to 1 s after the hit | 19/24 | 16/24 | 6/24 | 2/24 | 0 | about 0.8 s after |
| detector alone, 0.7 | 24/24 | 23/24 | 8/24 | 11/24 | 5, 6, 1, 3 of 60 | about 0.3 s before |

The check's rows are with the detector at 0.1. From 0.1 to 0.3 it woke on the same "hey ghost" takes, within one in every room condition (digital: 43, 46 and 44 of 48), and Parakeet ran 34, 7 and 2 times on the 6.1 h of read speech. Live, the check runs at the operator's first pause or 1 s after the hit, whichever comes first, so it falls between the two rows.

**Cost while passive:** one score takes 17.5 ms p50 on one thread, about 22% of a core all the time, on top of the VAD both paths run. Transcript wake adds nothing in a quiet room and a Parakeet run for every utterance in a busy one.

## Findings

1. **The detector hears "hey ghost"; the check is what loses it.** At the desk the detector caught every take. Parakeet, given "hey ghost" and a pause, wrote most of the rejected ones as "He goes.", "Hey goes." or "Okay goes."
   - Given a second more of the sentence it did better (19 vs 15 of 24), but the chime then comes later than transcript wake's.
   - On synthetic speech neither check beats transcript wake. That is ADR 0025's acceptance gate, so the operator's takes decide.
2. **Alone at 0.7, the detector made no false wakes in 6.1 h of read speech** and chimes before "ghost" has finished. What it does wake on is the near-misses above, 5 or 6 in 60 at the desk.
3. **It doesn't wait for the VAD.** Far from the mics it heard 11 of 24, where transcript wake heard 2: at about 2 dB above the room's noise the VAD rarely opens.
4. **Music on channel 0 is its weak spot** (8 of 24 at 0.7; channel 1 heard 14). Why isn't measured; channel 0 is the array's noise-suppressed beam.
5. **Against the pilot** (20,000 steps, 3,000 clips):
   - The full model scores higher, so its threshold sits higher.
   - At matched false wakes (full at 0.7, pilot at 0.5) it is the same at the desk and better far away (11 vs 6 of 24) and with music on channel 1 (14 vs 12).
   - The training run's own validation (7,500 synthetic positives, 21.7 h of negatives) gives 85% recall at 0.09 false accepts an hour at 0.66.
6. **`ghost.mic` heard nothing**, as in the review. The clips came from the desk sink, which the echo canceller subtracts. That says nothing about a person in the room.

## Changed

- `GHOST_WAKE_THRESHOLD` unset now means 0.3 with the check on and 0.7 with it off. Before, it was 0.3 either way, which alone woke on 9 of 60 desk negatives and twice in the read speech.
- The model is installed at `~/.cache/ghost/wake/hey_ghost.onnx` (sha256 `a2732403…`), the default path. Wake mode stays `transcript` until the operator changes it.

## Next

- **Score the operator's takes** (#8) at both settings.
- **Score windows in a stream.** Each score recomputes all 16 speech embeddings of its 2 s window, though 15 are the same as the last window's. The replay's scorer caches them and gets the same scores, at a fraction of the CPU.
- **A softer check.** If the takes confirm that Parakeet's "he goes" is what loses wakes, try one of these:
  - accept a high score at once and check only the middling ones (the replay's "hybrid" policy);
  - check with a longer window only when the first check fails.
