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
with a file trigger standing in for the wake word until the model is trained. Start
with [docs/architecture.md](docs/architecture.md) and the decisions in [docs/adr/](docs/adr/).

## Run it

Needs Python 3.12 (via `uv`) and PipeWire with the PulseAudio shim.

```bash
uv sync --all-packages
cp .env.example .env            # fill in the endpoint, token, PAT, and repo allowlist
set -a; . ./.env; set +a
uv run ghostd check             # config sanity
uv run ghostd run               # loads models (~10 s), then waits for a wake
uv run ghostd wake              # in another shell: simulate the wake word
```

Say "what needs my sign-off", talk it through, and "go to sleep" when done. Any
change Ghost proposes is read back and only happens after you say "confirm".

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
