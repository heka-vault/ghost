# 0039: The daemon is Rust, in one workspace with the desktop app's shell

- **Status:** accepted (decided 2026-10-28; built on the `rust-daemon` branch, heka-vault/ghost#39)
- **Date:** 2026-10-28
- **Supersedes:** ADR 0003 (Python for everything except the mobile app)
- **Amends:** ADR 0002 (Pipecat stays the model for the pipeline's ideas; its code goes), ADR 0009 (the daemon's unit becomes the headless mode; ADR 0038 puts it inside the desktop app)

The operator's two asks: the daemon ships as part of the desktop app, and, with time not a
factor, the best technical implementation. The Python daemon works (31,000 lines, 16,600 of
tests, twelve days old) and a rewrite makes no turn faster: the model is 78 % of a turn,
text-to-speech 15 %, the pipeline's own hops 0.02 s. What the language changes is the bundle,
the process model, the audio engine and whether Ghost owns its pipeline.

## Decision

**Rust, in one Cargo workspace with the Tauri shell.** The daemon is `crates/ghostd` with
`ghost-core`, `ghost-api`, `ghost-audio`, `ghost-infer`, `ghost-llm`, `ghost-rtc` and
`ghost-voice`; `apps/desktop/src-tauri` is a member. One `cargo build` makes both binaries the
bundle ships; the shell's bridge is typed by the same structs the daemon serializes.

**Two processes.** The shell supervises the daemon as a child (ADR 0038); the daemon holds the
vault's values and the microphone, and a WebKitGTK fault cannot take a session down. The same
binary runs headless under systemd by a flag.

**Why Rust.** No garbage collector on the audio path and a capture thread that never allocates;
native PipeWire through the project's own binding (one clock domain, no pulse shim); inference on
dedicated threads through ONNX Runtime; a pipeline with typed frames, bounded channels and
cancellation tokens; `zeroize` on the buffers that hold passwords; sum types and exhaustive
matching at every boundary that carries untrusted app text. Go was second: pion is the
strongest WebRTC outside a browser and every protocol Ghost speaks has an official Go package,
but it cannot share the shell's workspace and has no sum types. TypeScript on Electron was the
best organizational choice (one language with the desktop and the phone), not the best
technical one (one event loop, audio through a pipe, a pre-1.0 WebRTC). Zig is pre-1.0 with
no stable async and no SDK for anything Ghost speaks.

**Pipecat's ideas stay; its code goes.** Frames, processors, the user aggregator, interruption,
the speculative turn and the summarizer were the right shape. Ghost's pipeline is written for
one session and one transport at a time, and every lesson the Python daemon learned (every tool
call of a reply, the cut-off redo, EndFrame against cancel, the transfer's frame order, the
shared TTS) becomes a test before the stage that embodies it.

**The Python daemon is the oracle.** Fixtures are recorded from its own models (Parakeet's
transcripts, Smart Turn's verdicts, the wake scores, Kokoro's PCM, the model streams) and the
Rust daemon must match them: word for word, verdict for verdict, within 1e-3 where numbers are
compared. `make perf` on the Rust daemon must meet or beat the last Python report per stage.

## Consequences

- The port is one long-lived branch with many commits, merged only when it has proved itself
  (the merge gate in #39); the Python daemon runs the house from `main` until then and is removed
  on the branch before the merge. No staged cut-over.
- Every rule in CLAUDE.md carries unchanged: the gate, no undo, idle audio on the machine, the
  vault and redaction, verified webhooks, apps not MCP, deterministic first.
- `packages/api/openapi.json` stays the contract for the TypeScript client, generated from the
  Rust types.
- Python stays for the tools: the wake model's training and the benchmark scripts.
- CI gains `cargo fmt`, `clippy -D warnings`, `cargo nextest` and `cargo deny`; a known
  vulnerability in a dependency fails the build.
- The workstation builds the whole workspace: the WebKitGTK, PipeWire, espeak-ng and Opus
  development packages are installed; the Docker image is no longer needed for the shell.
