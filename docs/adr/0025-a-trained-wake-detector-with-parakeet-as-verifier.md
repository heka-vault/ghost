# 0025: A trained "hey ghost" detector wakes Ghost, and Parakeet confirms it

- **Status:** accepted 2026-11-26 (supersedes ADR 0010; see the amendment of that date)
- **Date:** 2026-10-22

ADR 0010 woke Ghost by transcribing everything said in the room and matching the text. The [wake review](../benchmarks/2026-10-21-wake.md) measured its limits:
- With strict matching it heard about 85% of wake phrases at the desk with no false wakes.
- The misses are Parakeet writing a bare "hey ghost" as "he goes", which no text rule can tell apart from someone saying "he goes".
- It wakes only after the whole utterance and its end-of-speech silence.
- It keeps the 0.6B model transcribing every utterance in the room.

We return to ADR 0005's plan, built the way Alexa and Siri are:
- **First stage:** a small acoustic model trained on the phrase listens in 80 ms frames and fires at a low threshold the moment "ghost" ends. It is `livekit-wakeword` (Apache-2.0, an openWakeWord-compatible ONNX model on onnxruntime).
- **Second stage:** Parakeet transcribes the audio around the hit, and `match_wake` (strict, 0.9) confirms it.
- Ghost wakes only when both agree. The words after the phrase still become the first turn.

## Consequences

- **Idle audio stays local.** Both stages run on this machine, and transcripts without a phrase are dropped as before.
- **Idle CPU drops.** Parakeet runs only on hits, not on every utterance in the room.
- **Training:**
  - It uses synthetic voices, room impulse responses, noise and named confusables ("hey guys", "he goes", "hey Gus"): about 16 GB of negative features and hours of CPU, run at low priority on the workstation.
  - The model file lives in the cache directory beside the speech models, not in the repository.
- **The operator's own takes are the test set.** They are recorded deliberately (`make record-takes`, [docs/wake-recording.md](../wake-recording.md)), at the desk, across the room and with music, and never taken from passive audio.
- **Acceptance gate.** This ADR becomes accepted only when the detector-plus-verifier:
  - hears more of the operator's takes than transcript wake does, and
  - wakes no more often than transcript wake on the replay rig (LibriSpeech, the room negatives, music).
- **The check can be turned off to try the detector alone** (`GHOST_WAKE_VERIFY=false`): the chime then plays the moment "ghost" ends. In detector mode (`GHOST_WAKE_MODE=onnx`) only "hey ghost" wakes Ghost; there is no transcript fallback, so living with it shows how the phrase works on its own.
- **No interface change.** The `WakeDetector` interface stays; the phrase list and the 0.9 matcher remain the verifier's rules.
- **Addressing Ghost by name has its own models** (every `ghost_*.onnx` beside `hey_ghost.onnx`: one model, or one per opener). It is trained on the openers "Ghost, I need you", "Ghost, let's work on", "Ghost, what is the status of" and "Ghost, what (or where) did we leave off on", and fires as the opener ends. Everything after "Ghost" is the first turn, so "Ghost, what is the status of the infra repo?" reaches the model as that whole request. Its check (`match_opening`) wants the name to open what was said, after one interjection at most ("so, Ghost"), so "the ghost story" isn't talking to Ghost. Both models share one pass of the audio features, and each has its own threshold.
- **A changed phrase needs a new model.** A phrase added in settings is honoured by the verifier only until a model for it is trained. Until then, transcript wake stays available as a setting.

## Amendment, 2026-11-12: the check of a weak hit takes Parakeet's spelling of "hey"

A score under the threshold but at the checked floor (`GHOST_WAKE_CHECK_THRESHOLD`, 0.3) is a weak hit, and Parakeet must confirm it. On `ghost.mic` Parakeet writes the operator's "Hey Ghost" as "A ghost." more than anything else, which the 0.8 match refuses. So the operator had to speak up to get past the threshold.

For a weak "hey ghost" hit, the check now also accepts a sentence of the reading that opens with the word "ghost", after at most one short interjection Parakeet writes for "hey" (`match_heys_spelling`). The name inside a sentence is still no match. The same rule gives back the request after "A ghost," once Ghost is woken.
- On the operator's recorded takes it heard 16 of 21 on `ghost.mic`, against 10.
- It added no false wake on any negative set ([2026-11-12 wake check](../benchmarks/2026-11-12-wake-check.md)).

The first frame at the floor still decides, though the score may still be rising, so a near miss that climbs past the threshold is checked. Waiting for the peak heard more takes, but it let confusables wake unchecked (issue #13).

Waking on the bare name wherever Parakeet reads it was measured as well and not taken. In the operator's speech, half the times "ghost" was said were product talk, sign-offs ("Later, ghost") or session commands ("Hang on, ghost"), so the name must open what was said. Inside the detector's hits, accepting it anywhere heard nothing more and woke on "Hang on, Ghost" and "Hang up ghost".

## Amendment, 2026-11-26: the detector ships, and wakes alone

The detector was trained on 2026-10-22 and woke the operator's desk from 2026-10-24, but it
only ever lived in that machine's cache (`~/.cache/ghost/wake/hey_ghost.onnx`); a release
never carried it. When Ghost was removed and reinstalled on 2026-11-25 the desk fell back
to transcript wake, and the operator's "Hey Ghost" went unheard with music on (#190).
`wake-probe` on the 2026-10-21 script measured transcript wake at 38 wakes near, 18 with
music, 8 far.

- **It ships.** `hey_ghost.onnx` is the fifth of the small model files
  (`ghost_infer::models::FILES`): fetched by `scripts/models.sh` from Ghost's own
  pre-release `wake-hey-ghost-1`, pinned by SHA-256, staged into the bundle. Retrained the
  same way as 2026-10-22 (livekit-wakeword, `conv_attention` medium, the look-alike
  negatives of `hey_ghost_full.yaml`) on a Runpod A40 (`.scratch/wake-train/pod/`).
- **Found in order:** `GHOST_WAKE_MODEL_PATH`; the operator's own in
  `~/.cache/ghost/wake/`; the shipped one (bundle, then `~/.cache/ghost/models/`). A
  retrained detector (ADR 0065) is chosen by `GHOST_WAKE_DETECTOR` as before.
- **The defaults change:** `GHOST_WAKE_MODE` is `onnx` and `GHOST_WAKE_VERIFY` false, so the
  detector wakes alone at 0.7, the operator's live setting since 2026-10-24 (every desk
  "hey ghost" and no false wake in 6.1 h of read speech, 2026-10-22). Transcript wake stays
  the fallback when the detector can't load, and the setting for anyone who prefers it.
- **Ghost by name, "Ghost, can you …" and "Ghost, what …"** (the operator, the same day):
  "Hey Ghost" wakes Ghost and it answers; "Ghost, can you open Chrome for me", "Ghost, what
  is the capital of Michigan?" are a request said with the name. Two opener models,
  `ghost_can_you.onnx` and `ghost_what.onnx`, trained as the 2026-10-23 openers were (one
  model per opener, the opener without the name and the name's sound-alikes as negatives),
  ship beside the detector. A hit is checked by Parakeet from 0.3 by default
  (`GHOST_WAKE_ADDRESS_VERIFY`, no longer following `GHOST_WAKE_VERIFY`), the request after
  the name is the first turn (no greeting), and Ghost waits for the operator to finish, plays
  its chime, then works. `OPENERS` is the trained ones only ("can you", "what", "what's");
  October's four had no model left. The transcript phrases ("yo ghost", "okay ghost" …) wake
  only in transcript mode.
- **The name without its "G"** (#192, the first live test the same day): one of three
  "Ghost, can you …" fired the model and Parakeet read "Oh, can you tell me the capital of
  Michigan?", with no "G" among its five best first tokens; other cuts of the audio read
  "Host". The array takes the onset, as it takes the "hey" of "Hey Ghost". So for an opener
  model's hit, the very first word heard counts as the name when it is "Ghost" without its
  onset ("oh", "o", "host", `ONSETLESS_NAME`) and an opener follows it. Only there: later in
  a sentence, without an opener, or "so" and "okay", it never does. The cost is a room's
  "Oh, what …" or "Oh, can you …" said as a whole utterance that also fires an opener model,
  which would wake Ghost.
