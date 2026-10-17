# 0002: Pipecat for the voice pipeline, not LiveKit

- **Status:** accepted
- **Date:** 2026-10-16
- **Supersedes:** original spec ADR-005 (managed LiveKit media plane) and the LiveKit Agents choice

Pipecat (BSD-2) runs the conversation: VAD, turn detection, STT, LLM, TTS, barge-in. On the desktop it uses a local audio transport, so there is no media server at all. Mobile later uses its peer-to-peer SmallWebRTC or WebSocket transport, which also needs no media server. Its turn detector, Smart Turn, is BSD-2 too, and it has first-class plugins for local Whisper/Parakeet-class STT and Kokoro.

## Considered options

- **Self-hosted LiveKit server + LiveKit Agents** (Apache-2.0). Works, but adds a media server to run and, for mobile, UDP ports or TURN to expose. Its newer turn-detector models are licensed for use only inside LiveKit Agents.
- **LiveKit Cloud.** Ruled out by ADR 0001.

## Consequences

- The agent is Python (ADR 0003).
- `livekit-wakeword` (Apache-2.0) is still usable for the wake word. It is a standalone ONNX model and does not need a LiveKit server.
