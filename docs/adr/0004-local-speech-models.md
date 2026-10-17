# 0004: Local speech on CPU: Parakeet TDT 0.6B v3 for STT, Kokoro for TTS

- **Status:** accepted
- **Date:** 2026-10-16

Measured on the workstation ([benchmark](../benchmarks/2026-10-16-speech-cpu.md)): Parakeet v3 (int8) transcribes a voice command in about 0.16 s and was the only fast model that got one-word commands such as "Confirm." right. Whisper medium and large-v3-turbo were more accurate overall but took 2.7–3.5 s per command, which alone uses up the 2 s first-reply target. Kokoro v1.0 produces the first sentence of a reply in about 0.37 s at roughly 4x real time and sounds far more natural than Piper. The LLM is Claude through an Anthropic-compatible endpoint. Models sit behind one pipeline config so any of them can be swapped, including for a cloud service, without touching skills or the domain.

## Considered options

- **Moonshine tiny/base.** Fastest (0.08–0.14 s) but misheard short commands.
- **Piper.** 10x faster than Kokoro but clearly more robotic; kept as a fallback voice.
- **Cloud STT/TTS (Deepgram, ElevenLabs, OpenAI).** Not needed at current measured speed; possible later behind the same config.

## Consequences

- Proper nouns (repo and org names, "Pipecat") need help: a vocabulary list and LLM correction against the allowlisted repositories.
- The benchmark used synthetic speech. Re-run on the real microphone, with music, before tuning.
