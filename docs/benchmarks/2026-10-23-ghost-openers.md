# "Ghost, <request>": detectors for addressing Ghost by name, 2026-10-23

**Question:** "Hey Ghost, <request>" already wakes Ghost with the request as the first turn. Can Ghost wake the same way on the name alone, as in "Ghost, what is the status of the infra repo?"? The operator chose four openers, trained as they are rather than as "Ghost" followed by anything:
- "Ghost, I need you"
- "Ghost, let's work on"
- "Ghost, what is the status of"
- "Ghost, what did we leave off on"

**Training:** `livekit-wakeword` 0.3.0, `conv_attention` medium, the hey_ghost recipe ([2026-10-22](2026-10-22-wake-detector.md)). It ran on Runpod GPU pods (A40, RTX A6000, L4) rather than the workstation, with `.scratch/wake-train/pod/`.

Three things changed from the hey_ghost run:
- **Punctuation reaches Piper.** espeak's phonemes had dropped it, so "Ghost, I need you" and "Ghost I need you" were the same clip. With it kept, the comma pauses about 0.12 s.
- **The longest openers fit the 2 s window.** Clip lengths are capped at a length scale of 1.1 (1.05 in v2), down from 1.25. Even at 1.1, 12% of the v1 positives were over 2 s, and the front of "Ghost" was cut from those.
- **The negatives include each opener without the name** ("let's work on"), the name inside a sentence ("the ghost story"), and sound-alikes ("Coast, I need you").

Models:
- **v1:** one model for every opener, plus "where did we leave off on". It is installed as `ghost_address.onnx`.
- **v2:** v1 with the automatic negatives that kept "ghost" dropped ("ghost let's work", "ghost i need"). It also added "what's the status of" and "where did we leave off". It wasn't better, and it isn't used.
- **Per opener:** `ghost_need`, `ghost_work`, `ghost_status`, `ghost_left`, one model each, on the hey_ghost recipe with 15,000 clips.

**Rig:**
- **Synthetic speech:** Kokoro (not Piper, which trained the models), 16 voices at three speeds, each clip with 2 s of silence on either side. It covers:
  - the trained openers, each followed by a request;
  - a full stop after the name ("Ghost. Let's work on …");
  - openers the models were not trained on;
  - 176 sentences that must not wake Ghost.
- **Room and read speech:** the [wake review](2026-10-21-wake.md)'s room recordings (780 sentences that must not wake Ghost) and 6.1 h of LibriSpeech.
- **The check:** Parakeet over 2.5 s before the hit to 1 s after. The name must open what was said (`match_opening`, 0.9).
- **Scripts:** `.scratch/wake-train/eval_address.py` and `eval_lk.py`. Several models are scored as one detector: the highest score per frame.

**Not tested:** the operator's voice, and the openers spoken in the room.

## Results

Clips that woke Ghost. "Checked" means the detector fires from 0.3 and Parakeet confirms the hit.

| Setup | Trained openers (80) | Pause after the name (48) | Reworded (80) | Must not wake (176) |
|---|---|---|---|---|
| v1, alone at 0.7 | 35 | 15 | 19 | 1 |
| v2, alone at 0.7 | 33 | 8 | 21 | 4 |
| per opener, alone at 0.7 | 36 | 28 | 21 | 9 |
| per opener + v1, alone at 0.7 | 50 | 30 | 23 | 9 |
| per opener + v1, alone at 0.6 | 61 | 35 | 32 | 11 |
| **per opener + v1, checked** | **70** | **42** | **38** | **0** |
| v1, checked | 66 | 36 | 35 | 0 |

For comparison, hey_ghost alone at 0.7 heard every "hey ghost" at the desk on the same kind of clips.

By opener, per opener + v1 (of 16):

| Opener | alone at 0.7 | checked |
|---|---|---|
| Ghost, I need you to look at the failing build. | 13 | 14 |
| Ghost, let's work on the wake word model. | 8 | 13 |
| Ghost, what is the status of the infra repo? | 9 | 13 |
| Ghost, what did we leave off on yesterday? | 10 | 15 |
| Ghost, where did we leave off on the desktop app? | 10 | 15 |

**False wakes outside the synthetic sound-alikes:**
- **Room recordings:** none of the 780 at 0.7; 1 at 0.5 and 0.6 ("I'll grab coffee, do you want anything?", over music).
- **LibriSpeech:** none at 0.4 and above; 1 at 0.2 and 0.3, which the check is there to catch.
- **Synthetic sound-alikes, alone at 0.7:** 8 were "Coast, I need you to look at this" (ghost_need) and 1 was "He goes, let's work on it later".

**Request:** in 78 of 80 trained clips, everything after "Ghost" (match_opening at 0.8) began with the opener's words. The two misses were Parakeet's spelling of the topic ("infrared" for "infra repo").

**End to end** (`.scratch/wake-train/smoke_address.py`: Silero VAD, the six models and Parakeet through `DetectorWakeProcessor`):
- With "hey ghost" alone at 0.7 and "Ghost, …" checked, each trained opener woke Ghost about 0.1 s before the sentence ended. The first turn was the request after the name.
- "The ghost story we read last night was great" did not wake it.
- "Hey Ghost, …" chimed 1.45 s before the end, unchecked as before.

**Cost while passive:** the six models share one pass of the audio features. One score takes 18.3 ms p50 against 17.4 ms for hey_ghost alone, so about 1% more of a core.

## Findings

1. **Openers are harder than "hey ghost" for this model.** The best single detector at the hey_ghost threshold heard 63% of the trained openers. What helps:
   - **One model per opener** raised "I need you" from 2 to 13 of 16 at 0.7.
   - **Keeping v1 alongside them** adds the "status" and "leave off" clips that it scores higher.
   - **v2's change didn't help.** Dropping the near-copy negatives left recall about the same and added false wakes.
2. **The check is what makes these usable.** From 0.3 with Parakeet it heard 88% of the trained openers and of those with a pause after the name. It turned down all 23 false hits: Parakeet writes "Ghost, …" correctly when words follow it, which a bare "hey ghost" doesn't get. So "hey ghost" can wake at once while "Ghost, …" is checked. The cost is when the chime plays: at the operator's first pause or 1 s after the hit, so about when they finish a short request rather than as the opener ends.
3. **These models learned wordings, not the name.**
   - Reworded openers that keep the trained words woke it: "what's the status of", "where did we leave off".
   - New openers did not: "Ghost, can you check the build" and "Ghost, let's look at the open issues" scored 0 of 16.
   - The name inside a sentence and the openers without the name scored near 0.
4. **The GPU does the generating and training; the CPU still does the rest.** Each run took 1 h 10 min to 2 h 10 min on a pod:
   - **Clip generation:** 50–65 clips/s on the GPU, against 3–4 on the workstation.
   - **Training:** 18 min, against 2 h 40 min on the workstation.
   - **Mixing in noise and echo, and feature extraction:** in livekit-wakeword these run on CPU only, so they took 35–55 min each on 9 cores and 1 h 40 min on the L4's 6.
   - **Total:** all six models cost $7.80.

## Changed

- `ghost_voice.wakeword`:
  - every `ghost_*.onnx` beside `hey_ghost.onnx` is a "Ghost, …" model: the name opens what was said, and the request is everything after it;
  - each model has its own threshold and can have its own check.
- `match_opening` in `ghost_voice.wake`.
- Settings: `GHOST_WAKE_ADDRESS_VERIFY` (unset: as `GHOST_WAKE_VERIFY`), `GHOST_WAKE_ADDRESS_THRESHOLD` (unset: 0.3 with the check, 0.7 without), and `GHOST_WAKE_ADDRESS_MODEL_PATH`.
- `~/.cache/ghost/wake/` holds `ghost_need`, `ghost_work`, `ghost_status`, `ghost_left` and `ghost_address` (v1) beside `hey_ghost`. They are listed in `SHA256SUMS`.

## Next

- Score the operator's own openers, at the desk and across the room, both ways.
- If more openers are wanted, train each as its own model. "Ghost, can you …" won't fire today.
