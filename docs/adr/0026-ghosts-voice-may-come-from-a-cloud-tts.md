# 0026: Ghost's voice may come from a cloud TTS; what the operator says stays local

- **Status:** proposed (amends ADR 0004)
- **Date:** 2026-10-22

The [cloud speech review](../benchmarks/2026-10-22-cloud-speech.md) found that a cloud text-to-speech service is the one speech service that clearly beats what runs here:
- **The voice.** It sounds better.
- **Steady first audio.** ElevenLabs Flash v2.5 starts in about 0.25 s whatever the sentence length. Kokoro took 0.54 s p50 and over 1 s on 7 of 94 turns.
- **Less CPU.** Synthesis moves off the workstation.

Cloud speech-to-text is no faster to a final transcript than Parakeet, and it would send the operator's voice off the machine.

So Ghost's voice may come from ElevenLabs Flash v2.5 over its multi-context websocket, chosen in the operator's settings, with Kokoro kept loaded as the fallback. Speech-to-text stays on Parakeet.

## Consequences

- **What leaves the machine.** Three things leave now: LLM calls, GitHub calls, and the text of Ghost's replies (which the LLM provider already has). No microphone audio leaves: passive, session and hold audio all stay local. A cloud speech-to-text would need its own ADR, and listening on hold (ADR 0021) stays local regardless.
- **Data handling.** ElevenLabs trains on what it receives unless the operator opts out (Settings → Data use), and keeps logs and backups for up to 30 days. Zero Retention Mode is Enterprise only.
- **The key.** `ELEVENLABS_API_KEY` comes from the secrets store, goes in a request header, and is never logged.
- **When ElevenLabs fails.** If the key is missing, the network is down, or the socket fails, Ghost speaks with Kokoro rather than going silent.
- **Line voices.** A line's voice (ADR 0014) names a Kokoro voice today. With ElevenLabs on, each line needs an ElevenLabs voice too, and the Kokoro one stays as its fallback.
- **Barge-in.** Pipecat's ElevenLabs service puts only the words actually spoken into the context after an interruption, using ElevenLabs' word timestamps.
- **Acceptance.** This ADR becomes accepted after the operator compares voices against Kokoro and first audio is measured from this desk.
