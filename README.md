# Ghost

Ghost is a voice-first assistant for triaging GitHub issues. Say "Ghost" at your desk, ask what needs your sign-off, talk through the options, and approve or request changes without opening the issue tracker. Every write to GitHub is read back to you and needs a spoken "confirm" before it happens.

"Ghost" is the codename. The original product documents call it **VoiceTriage**.

Ghost is a personal tool that runs entirely on your own Linux machine:

- **Wake word** detected locally. Idle audio never leaves the machine.
- **Speech-to-text and text-to-speech** run locally on CPU (Parakeet and Kokoro).
- **Conversation** is built on [Pipecat](https://github.com/pipecat-ai/pipecat). No media server and no cloud voice platform.
- **The LLM** is reached through any Anthropic-compatible endpoint.
- **GitHub** is reached with a fine-grained personal access token scoped to the repositories you choose.

It costs nothing to run beyond the LLM calls.

## Status

First increment in progress: the `uv` workspace exists, the write gate and the GitHub
issue skill are implemented with tests, and the desktop daemon runs the voice loop
with a local transcript matcher as the wake phrase until a wake model is trained. Start
with [docs/architecture.md](docs/architecture.md) and the decisions in [docs/adr/](docs/adr/).

## Run it

Needs Python 3.12 (via `uv`) and PipeWire with the PulseAudio shim.

```bash
make sync            # uv workspace
make echo-cancel     # echo-cancelled mic "ghost.mic" as a user service; without it Ghost hears itself
make env             # render ~/.config/ghost/env from the secrets store (or: cp .env.example .env and fill it in)
make check           # config sanity
make run             # loads models (~3 s warm), then listens for "hey ghost"; Ctrl+C stops it
make wake            # optional, from another shell: trigger a wake by hand
```

Say "Hey Ghost", or "Ghost, you there?" (`GHOST_WAKE_PHRASES` to change them). Words
in the same breath are the first request: "hey ghost, what needs my sign-off".

When Ghost does not react the way you expect, run `GHOST_DEBUG_AUDIO=1 make run`: it
logs what Parakeet heard (also while passive), VAD starts and stops, wake matches and
misses, barge-in decisions, and confirmation verdicts. It is off by default and logs
only to your terminal.

`make help` lists the rest (`test`, `lint`, `start`/`stop` for a background daemon,
`restore-volumes`, `service`). Non-secret settings such as `GHOST_GITHUB_MOCK=1` go in
`~/.config/ghost/settings.env`, which `make env` seeds and folds into the rendered file.

Say "what needs my sign-off", talk it through, and "go to sleep" when done. Any
change Ghost proposes is read back and only happens after you say "confirm".
Ctrl+C stops the daemon; "go to sleep" only ends the conversation.

Deploy as a user service with [infra/systemd/](infra/systemd/) and set up echo
cancellation with [infra/pipewire/](infra/pipewire/).

## Docs

| Doc | What |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Current design: components, how a conversation flows, how writes stay safe |
| [docs/adr/](docs/adr/) | Decisions, one per file |
| [docs/benchmarks/](docs/benchmarks/) | Measured speech-model performance on the target workstation |
| [docs/original/](docs/original/) | The original VoiceTriage PRD and technical specification, kept unchanged. Where they differ from the ADRs, the ADRs win |

## License

[MIT](LICENSE)
