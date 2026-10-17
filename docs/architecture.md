# Ghost architecture

**Status:** first increment under way, 2026-10-16. This replaces the deployment and stack parts of [original/technical-specification.md](original/technical-specification.md). The product behavior in [original/prd.md](original/prd.md) still applies unless an ADR says otherwise.

## Shape

One operator, one Linux workstation that is on 24/7 (ADR 0001). Everything runs there as `systemd --user` services started directly with `uv` (ADR 0009). Only two things leave the machine: LLM calls, and GitHub API calls.

```mermaid
flowchart LR
  subgraph WS[Linux workstation]
    MIC[Mic via PipeWire<br/>echo-cancelled source] --> WAKE[Wake word<br/>local ONNX, "Ghost"]
    WAKE -->|wake| PIPE[Pipecat pipeline]
    MIC --> PIPE
    PIPE --> SPK[Speaker / music ducking]
    subgraph PIPE_IN[pipeline]
      VAD[Silero VAD + Smart Turn] --> STT[Parakeet v3<br/>local]
      STT --> LLM[LLM + skill tools]
      LLM --> TTS[Kokoro<br/>local]
    end
    LLM --> SK[Skills<br/>github_issues, ...]
    SK --> DOM[domain<br/>proposals, confirmation,<br/>audit, policy]
    DOM --> DB[(SQLite WAL)]
  end
  LLM <-->|HTTPS| PROXY[Anthropic-compatible endpoint<br/>Claude]
  DOM -->|fine-grained PAT| GH[GitHub REST]
  SK -->|reads| GH
```

Mobile comes later (ADR 0005). It will be a second transport into the same pipeline (Pipecat SmallWebRTC or WebSocket), and how the phone reaches the house is decided then.

## Components

| Component | Responsibility |
|---|---|
| `ghostd` | Long-running desktop daemon. Owns the microphone, runs the wake-word model while passive, and on wake builds a Pipecat pipeline with a local audio transport. Ducks and restores music, plays the chime and the thinking sound, and returns to passive on "go to sleep" or idle timeout |
| `voice` | Builds the pipeline: VAD and turn detection, STT, the LLM service, and TTS behind one config, so models can be swapped (ADR 0004) |
| `skills` | Pluggable capabilities (ADR 0006). Each skill declares its read tools and the write operations it can `prepare`. `github_issues` is the first one |
| `domain` | The write gate (ADR 0007): proposals, confirmation checks, idempotent execution, readback, and the audit log. The only code that can execute a write |
| SQLite | Review checkpoints, proposals, operations, operation events, optional transcripts |

## Conversation flow

1. **Passive.** `ghostd` feeds microphone frames to the local wake-word model only. Nothing is transcribed, stored, or sent.
2. **Wake.** "Ghost" fires. `ghostd` plays a chime, ducks the configured music streams, and starts the pipeline.
3. **Dialogue.** Silero VAD and Smart Turn decide when you have finished speaking. Parakeet transcribes, the LLM answers or calls skill tools, and Kokoro speaks one sentence at a time. Barge-in stops playback.
4. **Sleep.** "Go to sleep" or the idle timeout (45 s default) ends the pipeline, restores each music stream to its exact previous volume, and returns to passive.

## Writes

The protocol from the original spec (section 8) stands, with one simplification. Speech-to-text runs inside our own process, so the backend observes the transcribed user turn directly. The spec's worry that a model-supplied "confirmation text" might be fabricated goes away: `domain` accepts a confirmation only from a transcript frame the pipeline produced for the current session, never from LLM output.

1. The LLM calls `prepare` on a skill: target, typed change set, rationale.
2. `domain` fetches the issue, stores an immutable proposal (expires after 60 s), and returns a preview for the agent to read aloud.
3. The next user turn must be a clear affirmative. `domain` checks the proposal, its expiry, and that the issue hasn't changed, then claims it atomically.
4. `domain` applies each step, reads the issue back, and records success, partial, or failed. Generated comments carry a hidden marker so a retry never duplicates them.
5. The agent speaks the verified result, not a guess.

## Repository layout

```text
ghost/
├── apps/
│   ├── ghostd/          # desktop daemon: wake word, ducking, pipeline lifecycle
│   └── mobile/          # later: Expo / React Native (ADR 0005)
├── packages/
│   ├── voice/           # Pipecat pipeline factory, Parakeet service, sounddevice transport
│   ├── skills/          # skill interface + github_issues (reads, prepare, writer)
│   └── domain/          # proposals, confirmation, gate, audit, policy, SQLite
├── infra/
│   ├── systemd/         # ghostd.service + env renderer
│   └── pipewire/        # echo-cancel config
└── docs/
```

A `uv` workspace at the root, Python 3.12. Import direction is strictly
`ghostd -> voice -> skills -> domain`; `domain` imports nothing from the others.

### How the gate is wired into the pipeline

```text
transport.input -> Parakeet STT -> ConfirmationProcessor -> user aggregator -> LLM -> Kokoro -> transport.output
```

`ConfirmationProcessor` sees every `TranscriptionFrame`. If the gate holds a pending
proposal for the session, the frame's text is the confirmation evidence: a clear
affirmative executes, anything else cancels. The verified outcome is appended to the
context as a system note, so the model's next sentence describes what actually
happened. The model's tools are the skills' read tools, one `prepare`, and
`go_to_sleep`. There is no execute tool.

The local audio transport is a `sounddevice` port of Pipecat's PyAudio one, so the
install needs no compiled extension; PortAudio comes with the wheel and talks to
PipeWire through the PulseAudio shim.

## Open items

- Wake word: train "Ghost" with `livekit-wakeword` and measure false wakes against music. A single short word wakes falsely more often than a two-word phrase, so compare it with "Hey Ghost" before settling (ADR 0005).
- Real-voice benchmark on the workstation mic, with music playing ([benchmarks](benchmarks/2026-10-16-speech-cpu.md) used synthetic clips).
- LLM default: `claude-sonnet-5-5` with `thinking: {"type": "between_tools"}` (0.77 s to first token; see the [benchmark](benchmarks/2026-10-16-speech-cpu.md#llm-time-to-first-token)).
- A dedicated LLM API key for Ghost, so it can be revoked on its own.
