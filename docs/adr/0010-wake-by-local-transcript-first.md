# 0010: Wake by local transcript matching before training a wake model

- **Status:** accepted
- **Date:** 2026-10-17

ADR 0005 planned a trained ONNX wake model for "Ghost". Training and measuring one is a spike of its own, and the desktop loop needed a hands-free start now. While passive, `ghostd` runs the same local VAD and Parakeet models the session uses and matches the transcript against a short list of phrases ("hey ghost", "ghost you there", ...). Transcripts without a phrase are discarded at the matcher; nothing is logged, stored, or sent. Words after the phrase become the first turn.

## Consequences

- The passive state now transcribes locally. The privacy rule in CLAUDE.md is restated as "frames go only to local models" rather than "nothing is transcribed". Audio still never leaves the machine.
- Costs continuous CPU for VAD and STT. If idle load or false wakes against music are unacceptable, the ONNX detector from ADR 0005 replaces this; the `WakeDetector` interface is unchanged.
- Phrases are plain word sequences, no fuzzy matching. Mis-hearings of "ghost" need a phrase variant, not code.
