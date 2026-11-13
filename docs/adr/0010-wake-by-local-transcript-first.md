# 0010: Wake by local transcript matching before training a wake model

- **Status:** accepted; amended by ADR 0032 (Ghost may start a session) and ADR 0064 (wake attempts are kept)
- **Date:** 2026-10-17

ADR 0005 planned a trained ONNX wake model for "Ghost". Training and measuring one is a spike of its own, and the desktop loop needed a hands-free start now. While passive, `ghostd` runs the same local VAD and Parakeet models the session uses and matches the transcript against a short list of phrases ("hey ghost", "ghost you there", ...). Transcripts without a phrase are discarded at the matcher; nothing is logged, stored, or sent. Words after the phrase become the first turn.

## Consequences

- The passive state now transcribes locally. The privacy rule in CLAUDE.md is restated as "frames go only to local models" rather than "nothing is transcribed". Audio still never leaves the machine.
- Costs continuous CPU for VAD and STT. If idle load or false wakes against music are unacceptable, the ONNX detector from ADR 0005 replaces this; the `WakeDetector` interface is unchanged.
- Phrases are plain word sequences, no fuzzy matching. Mis-hearings of "ghost" need a phrase variant, not code.

## Amendment, 2026-11-13: wake attempts are kept (ADR 0064)

"Transcripts without a phrase are discarded; nothing is logged, stored, or sent" now has one
exception. Each decided hit of the trained detector (ADR 0025), whether Ghost woke or
Parakeet's check turned it down, is kept on this computer unless the operator turned keeping
off: its audio (from 12 s before an accepted wake or 4 s before a refused hit to 1.5 s
after, from the stream the detector hears and the array's own), the detector's score curve
over it and what Parakeet read for it. On by default, said where a person sees it, capped,
deletable, never sent anywhere and never offered to the phone, an outside tool or a model
([ADR 0064](0064-ghost-keeps-its-wake-attempts-to-learn-the-operators-voice.md)). Passive
audio the detector did not score as a hit, and every transcript-wake transcript without a
phrase, are still dropped as before.
