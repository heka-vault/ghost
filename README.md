# Ghost

Ghost is a voice-first assistant for triaging GitHub issues. Say "Ghost" at your desk, ask what needs your sign-off, talk through the options, and approve or request changes without opening the issue tracker. You decide how much it does on its own: by default it labels and comments as you ask and tells you in passing, and asks before closing anything. Say "undo that" to take the last change back.

<p align="center">
  <img src="docs/screenshots/session-pending.webp" alt="The Ghost desktop app in a session: the transcript, and a change waiting for your okay with Confirm and Cancel" width="100%">
</p>

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

Say "Hey Ghost", or "Ghost, you there?" (`GHOST_WAKE_PHRASES` to change them; phonetic
near-misses such as "hey guys" count too, `GHOST_WAKE_FUZZY=0` for exact only). Words
in the same breath are the first request: "hey ghost, what needs my sign-off".

When Ghost does not react the way you expect, run `GHOST_DEBUG_AUDIO=1 make run`: it
logs what Parakeet heard (also while passive), VAD starts and stops, wake matches and
misses, barge-in decisions, and confirmation verdicts. It is off by default and logs
only to your terminal.

While it runs, `make status` shows the daemon's state and `make say TEXT="..."` types a turn
to it through the control socket (`$XDG_RUNTIME_DIR/ghost/api.sock`), which is what the
desktop app uses. `make help` lists the rest (`test`, `lint`, `start`/`stop` for a background daemon,
`restore-volumes`, `service`). Non-secret settings such as `GHOST_GITHUB_MOCK=1` go in
`~/.config/ghost/settings.env`, which `make env` seeds and folds into the rendered file.

Say "what needs my sign-off", talk it through, and "go to sleep" when done. How much
Ghost may do without asking is in `~/.config/ghost/policy.toml` (seeded by `make env`
from `infra/policy.example.toml`): per operation, `deny`, `confirm`, `announce`, or
`quiet`. Every write is logged and "undo that" reverses the last one for ten minutes.
Ctrl+C stops the daemon; "go to sleep" only ends the conversation.

Deploy as a user service with [infra/systemd/](infra/systemd/) and set up echo
cancellation with [infra/pipewire/](infra/pipewire/). To teach Ghost something new,
write an ability: a small package or a folder under `~/.config/ghost/abilities/`
([docs/abilities.md](docs/abilities.md)). GitHub issues are the first one.

## Desktop app

`apps/desktop` is a Tauri 2 app (React) that talks to the daemon over its socket. Speaking
is the product; the app is the companion: the live session with a Confirm card for changes
that need your okay, your saved lines and their voices, what Ghost may do on its own, the
audit log, abilities, and settings. It follows the system's light or dark theme.

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/session.webp" alt="A live session on the Accounting line"><br><sub><b>Session.</b> A live session, patched through to the Accounting line.</sub></td>
    <td width="50%"><img src="docs/screenshots/conversations.webp" alt="The Accounting line: voice, persona, memory and pinned notes"><br><sub><b>Conversations.</b> Each line keeps its own voice, persona and memory.</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/policy.webp" alt="Per-operation autonomy levels"><br><sub><b>What Ghost may do.</b> One level per operation, from not allowed to quietly.</sub></td>
    <td><img src="docs/screenshots/audit.webp" alt="The audit log with statuses and filters"><br><sub><b>Audit log.</b> Every change Ghost made or asked to make, and how it went.</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/settings.webp" alt="Settings: write-only secrets, model, GitHub"><br><sub><b>Settings.</b> Secrets are write-only: the app only learns set or not set.</sub></td>
    <td><img src="docs/screenshots/welcome.webp" alt="First-run wizard, prototype"><br><sub><b>First run</b> (prototype). Setup in eight steps, from model key to the first “Hey Ghost”.</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/review.webp" alt="Review queue, prototype"><br><sub><b>Review queue</b> (prototype). What is waiting on your sign-off, and why.</sub></td>
    <td><img src="docs/screenshots/tray.webp" alt="Tray status window and HUD, prototype"><br><sub><b>Tray</b> (prototype). Status, the waiting change and undo from the top bar.</sub></td>
  </tr>
</table>

<p align="center">
  <img src="docs/screenshots/mobile.webp" alt="The phone app prototype: home with a live session, and a decision Ghost is asking about" width="420"><br>
  <sub><b>Phone</b> (prototype). Start a session, answer by voice, end it. No always-on listening.</sub>
</p>

The prototypes are designs for screens in the [charter](docs/original/prd.md) that have no
API yet; they render only with mock data. Screenshots are taken from the mock scenarios.

Nothing needs installing to work on it: run the daemon from the checkout (`make run`) and
either `make desktop-dev` (the Tauri window with hot reload) or `make desktop-web` (the same
UI in a browser; Vite proxies the socket). Both talk to whichever daemon owns the socket,
packaged or not. In development, `?mock=<scenario>` (for example `?mock=pending` or
`?mock=busy`) runs the UI against a built-in mock daemon instead, and `#/gallery` shows every
component in every state. The design rules are in
[docs/design-system.md](docs/design-system.md). `make deb` builds the installable packages.

## Docs

| Doc | What |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Current design: components, how a conversation flows, how writes stay safe |
| [docs/adr/](docs/adr/) | Decisions, one per file |
| [docs/design-system.md](docs/design-system.md) | The desktop app's design system: tokens, components, page patterns, copy, mock scenarios |
| [docs/benchmarks/](docs/benchmarks/) | Measured speech-model performance on the target workstation |
| [docs/original/](docs/original/) | The original VoiceTriage PRD and technical specification, kept unchanged. Where they differ from the ADRs, the ADRs win |

## License

[MIT](LICENSE)
