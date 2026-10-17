# Ghost architecture

**Status:** first increment under way, 2026-10-16. This replaces the deployment and stack parts of [original/technical-specification.md](original/technical-specification.md). The product behavior in [original/prd.md](original/prd.md) still applies unless an ADR says otherwise.

## Shape

One operator, one Linux workstation that is on 24/7 (ADR 0001). Everything runs there as `systemd --user` services started directly with `uv` (ADR 0009). Only two things leave the machine: LLM calls, and GitHub API calls.

```mermaid
flowchart LR
  subgraph WS[Linux workstation]
    MIC[Mic via PipeWire<br/>echo-cancelled source] --> WAKE[Wake phrase<br/>local VAD + Parakeet, "hey ghost"]
    WAKE -->|wake| PIPE[Pipecat pipeline]
    MIC --> PIPE
    PIPE --> SPK[Speaker / music ducking]
    subgraph PIPE_IN[pipeline]
      VAD[Silero VAD + Smart Turn] --> STT[Parakeet v3<br/>local]
      STT --> LLM[LLM + skill tools]
      LLM --> TTS[Kokoro<br/>local]
    end
    LLM --> SK[Abilities<br/>github_issues, ...]
    SK --> DOM[core<br/>gate, policy, audit]
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
| `ghostd` | Long-running desktop daemon. Owns the microphone, runs the passive wake listener (local VAD and Parakeet matching a phrase, ADR 0010), and on wake builds a Pipecat pipeline with a local audio transport. Ducks and restores music, plays the chime and the thinking sound, and returns to passive on "go to sleep" or idle timeout |
| `voice` | Builds the pipeline: VAD and turn detection, STT, the LLM service, and TTS behind one config, so models can be swapped (ADR 0004) |
| `api` | The control API (ADR 0013): FastAPI on a user-owned Unix socket. Status and a WebSocket of events, wake and sleep, typed turns, the pending confirm-level change with Confirm and Cancel, undo, audit, abilities, the policy file, write-only secrets, devices and setup. `packages/api/openapi.json` is generated from it and `packages/client-ts` from that. Nothing passive crosses it |
| `core` | The ability API and the write gate (ADR 0007, 0011, 0012): discovery and loading of abilities, the operator's autonomy policy, proposals over ability-defined targets and changes, confirmation classifiers (Jev or regex), idempotent execution, readback, undo, and the audit log. The only code that can execute a write |
| `abilities/*` | Installable capabilities (ADR 0012). Each contributes a prompt paragraph, read tools, write operations with a risk class and a writer, and optionally turn interceptors, pipeline processors, listeners and CLI commands. `github_issues` is the first one; third-party abilities use the same entry point |
| SQLite | Review checkpoints, proposals, operations, operation events, optional transcripts |

## Conversation flow

1. **Passive.** `ghostd` runs a short local pipeline: VAD, Parakeet, phrase matcher. A transcript without a wake phrase is dropped where it is produced. Nothing is stored or sent.
2. **Wake.** "Hey Ghost", "Ghost, you there?" or another configured phrase (`GHOST_WAKE_PHRASES`) matches. `ghostd` plays a chime, ducks the configured music streams, and starts the pipeline. Words spoken after the phrase in the same breath become the first turn, so "hey ghost, what needs my sign-off" skips the greeting. `make wake` still triggers it by hand.
3. **Dialogue.** Silero VAD and Smart Turn decide when you have finished speaking. Parakeet transcribes, the LLM answers or calls skill tools, and Kokoro speaks one sentence at a time. Barge-in stops playback once the STT has heard at least two words from you (`GHOST_BARGE_IN_MIN_WORDS`); VAD alone never interrupts, so a leak of Ghost's own voice past the echo canceller cannot cut it off.
   A typed turn from the desktop app (`POST /session/say`) enters the same pipeline after
   the STT stage, so it meets the same tools and the same gate as speech.
4. **Sleep.** "Go to sleep" or the idle timeout (45 s default) ends the pipeline, restores each music stream to its exact previous volume, and returns to passive.

## Writes

Speech-to-text runs inside our own process, so the backend observes the transcribed
user turn directly. `core` accepts a confirmation only from a transcript frame the
pipeline produced for the current session, never from LLM output. How much Ghost does
without asking is the operator's call (ADR 0011):

1. The LLM calls `change` on an ability's operation: the operation's `prepare` turns the
   arguments into a target and a typed change set.
2. `core` checks the ability's own allowlist, then resolves the autonomy level for
   this ability, operation, and target from `~/.config/ghost/policy.toml` (`deny`,
   `confirm`, `announce`, `quiet`; a missing entry falls back to the operation's
   risk class).
3. `core` fetches the target through the ability's writer and stores an immutable
   proposal with that snapshot.
4. At `announce` or `quiet` the proposal is executed at once. At `confirm` it waits
   up to 60 s for the next user turn; the classifier (Jev when configured, else the
   regex allowlist, negation guard in both) must call it a clear yes, and the issue
   must not have changed since the snapshot. Any other turn cancels it.
5. Execution claims the proposal atomically, applies each step through the writer, reads
   the target back, and records success, partial, or failed. Anything created carries a
   hidden idempotency marker so a retry never duplicates it.
6. The model gets the verified result and says what happened in a few words, or asks
   one short question when confirmation is needed. "Undo that" reverses the last
   executed change set within ten minutes using the stored snapshot and the marker.

## Repository layout

```text
ghost/
├── apps/
│   ├── ghostd/          # desktop daemon: wake word, ducking, ability loading, pipeline lifecycle
│   └── mobile/          # later: Expo / React Native (ADR 0005)
├── packages/
│   ├── voice/           # Pipecat pipeline factory, Parakeet service, sounddevice transport, slots
│   ├── api/             # control API on a Unix socket; openapi.json is the contract for the apps
│   └── core/            # ability API, loader, context, gate, policy, confirmation, audit, SQLite
├── abilities/
│   └── github_issues/   # the first ability: issue reads, operations, GitHub writer, mock
├── infra/
│   ├── systemd/         # ghostd.service + env renderer
│   └── pipewire/        # echo-cancel config
└── docs/
```

A `uv` workspace at the root, Python 3.12. Import direction is strictly
`ghostd -> {voice, api} -> core` and `abilities/* -> core`; `core` imports nothing from the
others, and nothing imports an ability by name: `ghostd` finds them through the
`ghost.abilities` entry point group and `~/.config/ghost/abilities/`. See
[abilities.md](abilities.md) to write one.

### How the gate is wired into the pipeline

```text
transport.input -> [audio_in] -> Parakeet STT -> InterceptorProcessor -> ConfirmationProcessor -> [after_stt]
  -> user aggregator -> [before_llm] -> LLM -> [after_llm] -> thinking sound -> [before_tts] -> Kokoro -> [audio_out]
  -> transport.output
```

The bracketed names are slots where enabled abilities' processors are spliced in, one
fresh instance per session. `InterceptorProcessor` offers each user turn to the
abilities' turn interceptors first (and to Ghost's own fixed intents: "go to sleep",
"never mind" while a change is pending, "undo that"); a handled turn is spoken to or
acted on directly and never reaches the model.

`ConfirmationProcessor` sees every `TranscriptionFrame`. If the gate holds a pending
proposal for the session, the frame's text is the confirmation evidence, judged with
Ghost's last reply as context: a clear yes executes, anything else cancels. Transcripts
that arrive while Ghost is still speaking are not evidence at all: the operator has
not heard the question yet, and the text may be Ghost's own voice. The verified
outcome is appended to the context as a system note, so the model's next sentence
describes what actually happened. The model's tools are the abilities' read tools,
`change`, `undo`, and `go_to_sleep`. There is no tool that bypasses the gate.

The local audio transport is a `sounddevice` port of Pipecat's PyAudio one, so the
install needs no compiled extension; PortAudio comes with the wheel and talks to
PipeWire through the PulseAudio shim. Ghost listens on `ghost.mic`, the output of
PipeWire's WebRTC echo canceller running as its own user service ([infra/pipewire](../infra/pipewire/README.md)). The
module takes the speaker sink as its reference, so what Kokoro plays is subtracted
from what the microphone hears before VAD or STT see it. Device names are rendered
into the user's config at install time and never committed.

## Open items

- Wake word: the transcript matcher costs continuous local STT while passive. Measure its CPU and false wakes against music; if either is a problem, train "Hey Ghost" with `livekit-wakeword` for the `OnnxWakeDetector` (ADR 0005, ADR 0010).
- Real-voice benchmark on the workstation mic, with music playing ([benchmarks](benchmarks/2026-10-16-speech-cpu.md) used synthetic clips).
- LLM default: `claude-sonnet-5-5` with `thinking: {"type": "between_tools"}` (0.77 s to first token; see the [benchmark](benchmarks/2026-10-16-speech-cpu.md#llm-time-to-first-token)).
- A dedicated LLM API key for Ghost, so it can be revoked on its own.
