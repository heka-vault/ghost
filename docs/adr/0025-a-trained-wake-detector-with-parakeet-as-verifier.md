# 0025: A trained "hey ghost" detector wakes Ghost, and Parakeet confirms it

- **Status:** proposed (supersedes ADR 0010 once accepted)
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
- **A changed phrase needs a new model.** A phrase added in settings is honoured by the verifier only until a model for it is trained. Until then, transcript wake stays available as a setting.
