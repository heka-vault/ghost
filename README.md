# Ghost

Ghost is a voice-first assistant for triaging GitHub issues. Say "Ghost" at your desk, ask what needs your sign-off, talk through the options, and approve or request changes without opening the issue tracker. You decide how much it does on its own: by default it labels and comments as you ask and tells you in passing, and asks before closing anything. Change your mind and Ghost changes it back, the same way.

<p align="center">
  <img src="docs/screenshots/session-pending.webp" alt="The Ghost desktop app in a session: the transcript, and a change waiting for your okay with Confirm and Cancel" width="100%">
</p>

Ghost is the name it goes live under. The original product documents use the working title **VoiceTriage**.

Ghost is a personal tool that runs entirely on your own Linux machine:

- **Wake word** detected locally. Idle audio never leaves the machine.
- **Speech-to-text and text-to-speech** run locally on CPU (Parakeet and Kokoro).
- **Conversation** is built on [Pipecat](https://github.com/pipecat-ai/pipecat). No media server and no cloud voice platform.
- **The LLM** is one model connection: Anthropic, OpenRouter (sign in), your Claude subscription through CLIProxy, OpenAI, a local Ollama or LM Studio, or any address that speaks Anthropic's or OpenAI's API (ADR 0037).
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
make setup           # once: uv workspace, pnpm install, Ghost's folder, the echo-cancelled mic "ghost.mic"
make dev             # the daemon and the desktop app from this checkout, logging to this terminal; Ctrl+C stops both
make reload          # from another terminal, after a Python change: restarts the daemon, not the app
```

The desktop UI reloads on save and a Rust change rebuilds the window; only Python needs
`make reload`. `make daemon` and `make desktop` run one half each, in two terminals if you
prefer. Only one Ghost may have the microphone, so `make dev` stops the `ghostd` user
service (your always-on Ghost, below) and `make service-on` starts it again.

Say "Hey Ghost", or "Ghost, you there?" (`GHOST_WAKE_PHRASES` to change them; phonetic
near-misses such as "hey guys" count too, `GHOST_WAKE_FUZZY=0` for exact only). Words
in the same breath are the first request: "hey ghost, what needs me?".

When Ghost does not react the way you expect, run `GHOST_DEBUG_AUDIO=1 make daemon`: it
logs what Parakeet heard (also while passive), VAD starts and stops, wake matches and
misses, barge-in decisions, and confirmation verdicts. It is off by default and logs
only to your terminal.

While it runs, `make status` shows the daemon's state and `make say TEXT="..."` types a turn
to it through the control socket (`$XDG_RUNTIME_DIR/ghost/api.sock`), which is what the
desktop app uses. `make help` lists the rest, grouped by what you are doing (`ci` before a push, `start`/`stop` for a background daemon).
Non-secret settings are in `~/.config/ghost/settings.env`, which `make env` seeds.

Passwords (every API key, token and account password Ghost holds) are in Ghost's own vault,
`~/.local/share/ghost/passwords.vault`, encrypted, its key in your login keyring (ADR 0036).
Save them on the desktop's Settings › Passwords, or with
`uv run ghostd passwords set ANTHROPIC_AUTH_TOKEN` (the value is read without echo);
`make passwords` lists them by name. A value is never shown again, and anything an app
sends back with one in it has it replaced before Ghost reads it. Coming from heka-vault or
the old `~/.config/ghost/env` and `apps.env`? `make passwords-import` moves them into the
vault once and deletes the plain files.

The model is picked in the desktop app under Settings › Model (ADR 0037): a provider
(`GHOST_LLM_PROVIDER`: `anthropic`, `openrouter`, `cliproxy`, `openai`, `ollama`, `lmstudio`
or `custom`), its model from the provider's own list (`GHOST_LLM_MODEL`), and its password,
kept in the vault like any other and referred to by name (`ANTHROPIC_API_KEY`,
`OPENAI_API_KEY`, and so on, or `GHOST_LLM_PASSWORD`). OpenRouter is a sign-in, with nothing
to copy. Test connection there times the first token. Every session, compaction and a
watch's model step use it, over the Anthropic Messages API or an OpenAI-compatible one, and
a saved change applies from the next session. Older settings still work: without a
provider, `GHOST_LLM_ENDPOINT=openrouter` is OpenRouter with the `OPENROUTER_API_KEY`
password, and otherwise `ANTHROPIC_BASE_URL` with `ANTHROPIC_AUTH_TOKEN`. `make check`
prints the provider and model in use, never a password. `make latency` shows where recent turns
spent their time ([docs/benchmarks/latency-harness.md](docs/benchmarks/latency-harness.md)).

The phone app joins over the home LAN only (ADR 0016). Set `GHOST_PHONE_LISTEN` in
`settings.env` to the workstation's LAN address and a port, for example
`GHOST_PHONE_LISTEN=192.168.1.20:8765`, restart, and pair from the desktop app (it shows
a six-digit code for two minutes). If `ufw` is on, allow the port and UDP from the LAN,
because aiortc picks its UDP ports at random:

```bash
sudo ufw allow from 192.168.1.0/24 to any port 8765 proto tcp
sudo ufw allow from 192.168.1.0/24 proto udp
```

Say "what needs me?", talk it through, and "that's all" when done. Ghost uses every
app you connect: it looks things up, makes the changes you ask for, and says what it did.
What you want blocked, or asked about first, is in `~/.config/ghost/policy.toml` (seeded
by `make env` from `infra/policy.example.toml`): `block`, `ask`, `quiet` and `allow`
rules over an app, a tool or its arguments, and a `mode` (`open`, `ask`, `read-only`) for
the rest. Every change is logged; to reverse one, ask Ghost for the opposite change. The
connected apps are in `~/.config/ghost/apps.json` (seeded from `infra/example/` with a
pretend GitHub); `ghostd check` and the desktop's Apps page show what each one gave.
Ctrl+C stops the daemon; a goodbye only ends the conversation.

Deploy as a user service with [infra/systemd/](infra/systemd/) and set up echo
cancellation with [infra/pipewire/](infra/pipewire/). To teach Ghost something new,
connect an app ([docs/apps.md](docs/apps.md)): from the desktop's catalog (sign in, a
password, or a package that runs on this computer), by address, or by pasting another tool's
config. Ghost's folder holds the rules every agent reads, what it watches, what it makes
and its agents ([docs/folder.md](docs/folder.md)); skills are folders of instructions
([docs/skills.md](docs/skills.md)). `ghost-mock-github` stands in for GitHub before you
connect the real one.

## Desktop app

`apps/desktop` is a Tauri 2 app (React) that talks to the daemon over its socket. Speaking
is the product; the app is the companion: the live session with a Confirm card for changes
that need your okay, what needs you, your agents and their voices, what Ghost watches, the
apps you connect and the catalog to connect more, permissions, the audit
log, skills and settings. It follows the system's light or dark theme.

<table>
  <tr>
    <td width="50%"><img src="docs/screenshots/session.webp" alt="A live session with the Accounting agent"><br><sub><b>Session.</b> A live session, transferred to Accounting.</sub></td>
    <td width="50%"><img src="docs/screenshots/conversations.webp" alt="The Accounting agent: voice, persona, memory and pinned notes"><br><sub><b>Agents.</b> Each agent keeps its own voice, persona and memory.</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/policy.webp" alt="Permissions"><br><sub><b>Permissions.</b> Everything runs unless you block it or want to be asked first.</sub></td>
    <td><img src="docs/screenshots/audit.webp" alt="The audit log with statuses and filters"><br><sub><b>Audit log.</b> Every change Ghost made or asked to make, and how it went.</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/settings.webp" alt="Settings: write-only passwords, model, GitHub"><br><sub><b>Settings.</b> Passwords are write-only: the app only learns set or not set.</sub></td>
    <td><img src="docs/screenshots/welcome.webp" alt="First-run wizard, prototype"><br><sub><b>First run</b> (prototype). Setup in eight steps, from the model’s password to the first “Hey Ghost”.</sub></td>
  </tr>
  <tr>
    <td><img src="docs/screenshots/review.webp" alt="Review queue, prototype"><br><sub><b>Review queue</b> (prototype). What is waiting on your sign-off, and why.</sub></td>
    <td><img src="docs/screenshots/tray.webp" alt="Tray status window and HUD, prototype"><br><sub><b>Tray</b> (prototype). Status and the waiting change from the top bar.</sub></td>
  </tr>
</table>

<p align="center">
  <img src="docs/screenshots/mobile.webp" alt="The phone app prototype: home with a live session, and a decision Ghost is asking about" width="420"><br>
  <sub><b>Phone</b> (prototype). Start a session, answer by voice, end it. No always-on listening.</sub>
</p>

The prototypes are designs for screens in the [charter](docs/original/prd.md) that have no
API yet; they render only with mock data. Screenshots are taken from the mock scenarios.

Nothing needs installing to work on it: `make dev` runs the daemon and the Tauri window
from the checkout, and `make desktop-web` serves the same UI in a browser (Vite proxies the
socket). Both talk to whichever daemon owns the socket. In development, `?mock=<scenario>` (for example `?mock=pending` or
`?mock=busy`) runs the UI against a built-in mock daemon instead, and `#/gallery` shows every
component in every state. The design rules are in
[docs/design-system.md](docs/design-system.md). `make deb` builds the installable packages.

To try a commit as your real Ghost, `make deploy` builds HEAD into
`~/.local/share/ghost/runtime/<commit>/`, runs `ghostd check` against `~/.config/ghost`, and
only then switches the `ghostd` user service to it and restarts it; no root and no .deb.
`make deploy-rollback` goes back to the build before. `make service-on` and `make service-off`
start and stop it, at login too.

Changes go to `main` through pull requests; the `ci` workflow (lint, tests, schema and client
current, typecheck, the desktop build and the Tauri shell) must pass before one merges.

Versions are dates, `YYYY.M.N` (the Nth release of the month, from 0). `make version V=next`
writes the next one everywhere, in a PR; once it merges, `make release` on `main` tags it.
The sidebar says which Ghost is running, for example "Ghost 2026.10.0 · dev a1b2c3d", and
`ghostd version` says the same. See [docs/releasing.md](docs/releasing.md).

## Docs

| Doc | What |
|---|---|
| [docs/architecture.md](docs/architecture.md) | Current design: components, how a conversation flows, how writes stay safe |
| [docs/adr/](docs/adr/) | Decisions, one per file |
| [docs/design-system.md](docs/design-system.md) | The desktop app's design system: tokens, components, page patterns, copy, mock scenarios |
| [docs/releasing.md](docs/releasing.md) | Version numbers, how a release is cut, and how each build says what it is |
| [docs/benchmarks/](docs/benchmarks/) | Measured speech-model performance on the target workstation |
| [docs/original/](docs/original/) | The original VoiceTriage PRD and technical specification, kept unchanged. Where they differ from the ADRs, the ADRs win |

## License

[MIT](LICENSE)
