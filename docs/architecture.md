# Ghost architecture

**Status:** first increment under way, 2026-10-16. This replaces the deployment and stack parts of [original/technical-specification.md](original/technical-specification.md). The product behavior in [original/prd.md](original/prd.md) still applies unless an ADR says otherwise.

## Shape

One operator, one Linux workstation that is on 24/7 (ADR 0001). Everything runs there as `systemd --user` services started directly with `uv` (ADR 0009). Only two kinds of thing leave the machine: LLM calls, and calls to the apps the operator connected (ADR 0034).

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
    LLM -->|reads| SK[Apps<br/>github, linear, ...]
    LLM -->|changes| DOM[core<br/>gate, permissions, audit]
    DOM --> SK
    DOM --> DB[(SQLite WAL)]
  end
  LLM <-->|HTTPS| PROXY[Anthropic-compatible endpoint<br/>Claude]
  SK -->|signed in, or a key| EXT[GitHub, Linear, ...]
```

The phone app (`apps/mobile`, Expo, Android) is a second transport into the same pipeline (ADR 0005, ADR 0013), so the gate, the policy and Jev apply unchanged. It never uses the control API. A question from Ghost is answered by voice on the phone: there is no Confirm button there. Its screens read a small `Desk` interface (`apps/mobile/src/desk/types.ts`). With the screen off a session keeps the mic through a microphone foreground service (`apps/mobile/modules/ghost-call`, a local Expo module) with an End action in its notification, and routes audio to a car or headset when one is connected. It reaches the house through the phone listener below (ADR 0016): on the LAN directly, and from anywhere else through the operator's own WireGuard network, where the desk and the phone are peers of a hub that forwards only between the two (ADR 0020). Nothing public is added: no STUN, no TURN, no relay; the tunnel is the operator's.

### Apps

The operator connects an app to Ghost once and every department reaches it (ADR 0034).
`~/.config/ghost/apps.json` lists them: a URL Ghost signs in to (OAuth, `ghost_core.signin`,
tokens in a 0600 file per app, the browser back on a loopback address only while a sign-in
is open) or sends a key to, or a command it runs on this computer; secrets are `${NAME}`
from the environment, and the keys the desktop keeps for apps are in `apps.env` beside
`env`. The desktop's Apps screen connects them from Ghost's own copy of the public app
registry (`ghost_core.catalog`: the first-party apps first, then the rest in the
background, refreshed at most hourly), by address, or from another tool's pasted config.
`ghostd` connects them at start (`ghost_core.apps.Apps`) and one connected later joins
the gate at once and the tools from the next session. Every tool is offered under the
app's name: one the app marks read-only is a read, called directly; every other tool is a
change, which goes to the gate. An app that fails to connect, or wants a new sign-in, is on
the waiting list and the Apps screen and is skipped. Everything an app sends is data: the
model sees the first sentence of each description, results are trimmed, and nothing in
them changes a permission. See [apps.md](apps.md).

### Permissions

Everything runs unless the operator says otherwise (ADR 0034). `[permissions]` in
`policy.toml` holds `block`, `ask`, `quiet` and `allow` rules over an app, a tool, or a
tool's arguments (`github.issue_write(state=closed)`, `github.*(repo!=site)`), and the
`mode` for what no rule names (`open`, `ask`, `read-only`). The most careful match wins.
A department's `department.json` adds `block`, `ask` and `quiet` rules for its own
sessions and never `allow` (`Department.policy`, `WriteGate.policy_for`). A tool an
unconditional `block` names is not offered; a rule with argument patterns refuses the
calls it matches. Ghost's own operations (`conversations.*`, `skills.*`, `departments.*`,
`outputs.*`) take the same rules. The desktop edits them on the Permissions screen and per
tool on an app's page; only the `[permissions]` table of the file is rewritten
(`ghost_core.policy.write_permissions`).

### Skills

A skill is a `SKILL.md` folder (ADR 0023, ADR 0024), read nearest first: the department's own (`skills/`, `.agents/skills/`, `.claude/skills/` under its folder or its `root`), the operator's `~/.agents/skills/`, then Ghost's own `~/.config/ghost/skills/`. `ghost_core.skills` finds them, puts an index (name and one line each) in the prompt, and offers `read_skill`, which returns a body or one of the files beside it, trimmed and inside the folder only. A `SKILL.md` that will not parse or whose name is not its folder's is a problem the operator sees and the model does not. Adding and removing Ghost's own are gate operations (`skills.add`, `skills.remove`), so the desktop's drag-and-drop and "Ghost, add the invoicing skill from this folder" are the same audited write. Skill text shapes what the model does, never what it may do; nothing in a skill is run. See [skills.md](skills.md).

### Departments and agents

A department is a folder (ADR 0024): `~/.config/ghost/departments/<slug>/` with `AGENTS.md` (its rules, in every agent's prompt), `department.json` (title, aliases, voice, an optional `root` whose files are read in place, and the permissions that narrow what its agents may do), a shared `memory.md`, `skills/`, `docs/` and `agents/`; Ghost is the config folder itself (`ghost_core.departments`). An agent is a conversation (ADR 0014) hired into a department (`conversations.create`), moved between them with its folder, memory and transcript (`conversations.move`; a running session follows), retired and rehired. A session in a department is built from every connected app, less what its permissions block, and what its folder gives (`Skills.source`, the department's `read_department_doc` and the agents' tools): every read tool is registered once and dispatched to the active agent's reach, and a transfer swaps the prompt, the voice and the advertised tools. Ghost writes into a folder only through the gate: an agent's files, the shared memory (`departments.remember`) and a rule appended verbatim to `AGENTS.md` (`departments.add_rule`, `ghost_core.department_tools`); never `department.json`. `infra/templates/` ships three folders to copy; `ghost-scripts` runs a skill's scripts as an app whose `run` is a change through the gate. See [departments.md](departments.md).

### The phone listener

`ghostd` serves a second, narrow HTTP app for the phone when `GHOST_PHONE_LISTEN` is set (one or more `host:port`, comma separated, bound to exactly those addresses: the LAN one first, then the tunnel one; off when empty). The first must bind at start; a later one whose interface isn't up yet is retried every few seconds. `/pair` and `/hello` carry the whole list, so the phone keeps every address and uses the first that answers. It is not the control API, shares no routes with it, and doesn't proxy it (`ghostd.phone`). Six routes: `POST /pair` spends a six-digit code the desktop app got from the control API's `POST /phone/pairing`; `GET /hello`, `GET /audit` and `GET /conversations` return the control API's `Status`, `AuditPage` and the live threads, built by the same functions (`ghost_api.status_snapshot`, `ghost_api.audit_page`, `ghost_api.conversations.conversation_view`); `POST`/`PATCH /offer` is WebRTC signalling, and the offer may name the thread the session opens in (the phone's Home lists them under the ghost; a thread called directly greets as itself); `POST /end` ends the phone's session. Every route but `/pair` is HMAC-signed both ways by a middleware, with a 60 s clock window and a nonce cache; the paired device and its secret are in `~/.local/share/ghost/phone.json` (0600). The wire format is [phone-protocol.md](phone-protocol.md).

An accepted offer becomes a `SmallWebRTCConnection` with no ICE servers, which the listener hands to the daemon. The passive loop waits on it next to the wake detector: it cancels the wake task, which releases the desk mic, and runs a session whose transport is a `SmallWebRTCTransport` (16 kHz in, Kokoro's 24 kHz out, the thinking sound mixed into the phone's track). A phone session doesn't duck music or chime on the desk, and `Status.session_origin` is `phone`. An offer while any session runs, or while the models load, is refused (409 `busy`, 503 `not_ready`). The session ends when the peer closes or stops its keepalive, on `POST /end`, or on RTVI `disconnect-bot`, and its ending closes the peer. Phone sessions are ended with an EndFrame, not cancelled: Pipecat leaves a processor that saw a CancelFrame refusing frames, and Kokoro's processor is shared by every session.

The data channel carries RTVI through `ghost_voice.rtvi.GhostRTVIProcessor`, which acts only on `client-ready` and `disconnect-bot` (no typed text, raw audio or function results), with an observer that sends `bot-ready` and the speaking events but no transcripts, LLM text, metrics or logs. The daemon forwards its own event bus (`status`, `turn`, `pending`, `outcome`, `conversation`; never `log`) to the phone as `server-message`s, the same JSON the control API's WebSocket sends.

## Components

| Component | Responsibility |
|---|---|
| `ghostd` | Long-running desktop daemon. Owns the microphone, runs the passive wake listener (local VAD and Parakeet matching a phrase, ADR 0010), and on wake builds a Pipecat pipeline with a local audio transport. Ducks and restores music, plays the chime and the thinking sound, and returns to passive on a goodbye or the idle timeout; "hang on, Ghost" holds the session until it is addressed by name again (ADR 0021) |
| `voice` | Builds the pipeline: VAD and turn detection, STT, the LLM service, and TTS behind one config, so models can be swapped (ADR 0004) |
| `api` | The control API (ADR 0013): FastAPI on a user-owned Unix socket. Status and a WebSocket of events, wake and sleep, typed turns, the pending change with Confirm and Cancel, audit, the apps (connect, disconnect, sign in) and the catalog, the permissions and the policy file, write-only secrets, devices and setup. `packages/api/openapi.json` is generated from it and `packages/client-ts` from that. Nothing passive crosses it |
| `core` | The write gate and what feeds it (ADR 0007, 0011, 0024, 0034): tool sources (`ghost_core.source.ToolSource`), the apps (`ghost_core.apps`), their catalog and sign-in (`ghost_core.catalog`, `ghost_core.signin`), the department folders (`ghost_core.departments`, `ghost_core.department_tools`), the operator's permissions (`ghost_core.permissions`, `ghost_core.policy`), proposals over typed targets and change sets, the confirmation classifier, the audit store, and the agents (ADR 0014). Imports nothing from the other packages. |
| `core/conversations` | The agents (ADR 0014, ADR 0024): the registry mirrored from the folders, the transcript, every compaction version, and the files per agent under its department's `agents/<slug>/` (`agent.json`, `persona.md`, `memory.md`, `notes.md`). Their mutations are gate operations under `[conversations]` |
| SQLite | Proposals, operations, operation events, the conversation registry, session transcripts, compaction versions |

## Conversation flow

1. **Passive.** `ghostd` runs a short local pipeline: VAD, Parakeet, phrase matcher. A transcript without a wake phrase is dropped where it is produced. Nothing is stored or sent.
2. **Wake.** "Hey Ghost", "Ghost, you there?" or another configured phrase (`GHOST_WAKE_PHRASES`) matches. With `GHOST_WAKE_MODE=onnx` the trained detectors listen instead (ADR 0025, `ghost_voice.wakeword`): "hey ghost", and, when its model is installed, Ghost addressed by name with a request ("Ghost, what is the status of …"). A model fires as its phrase ends, Parakeet checks the hit (`GHOST_WAKE_VERIFY`), and the chime plays then, while the rest of the sentence is still being heard. `ghostd` plays a chime, ducks the configured music streams, and starts the pipeline. Words spoken after the phrase in the same breath become the first turn, so "hey ghost, what needs me?" skips the greeting. `make wake` still triggers it by hand.
3. **Dialogue.** Silero VAD and Smart Turn decide when you have finished speaking; Ghost waits a short grace in case you go on, and the model starts answering during it, its answer held until the turn is confirmed and dropped if you keep talking (`GHOST_TURN_SPECULATE`; a held answer that reaches a tool call is withdrawn and the confirmed turn makes the call). Parakeet transcribes, the LLM answers or calls skill tools, and Kokoro speaks a clause at a time, so the voice starts after the first few words (`GHOST_TTS_CLAUSES`). Barge-in stops playback once the STT has heard at least two words from you (`GHOST_BARGE_IN_MIN_WORDS`); VAD alone never interrupts, so a leak of Ghost's own voice past the echo canceller cannot cut it off. While Ghost speaks, Parakeet re-reads your unfinished speech every 0.3 s (`GHOST_BARGE_IN_INTERIM_SECS`), so you needn't pause first; those interim words only ever decide barge-in, and the turn, the transcript and the model get the final transcript. Every tool call in a reply runs, not only the last (Pipecat's Anthropic service keeps one); and the model's output cap (`GHOST_LLM_MAX_TOKENS`, 2000 tokens) counts tool calls too, so a reply that hits it is redone in smaller steps rather than losing them (`ghost_voice.llm`).
   A typed turn from the desktop app (`POST /session/say`) enters the same pipeline after
   the STT stage, so it meets the same tools and the same gate as speech.
4. **Transfer.** Every session opens in `ghost`. "Transfer me to accounting" (a department's name lands on its default agent) is an LLM tool: Ghost's voice announces it, a short tone plays, the TTS voice and the system prompt change (Ghost's rules, the department's rules and shared memory, the agent's persona and memory), the tools become what the department reaches, and the context is replaced with that agent's rolling summary, pinned notes and recent turns; the agent greets with a one-line recap. "Back to Ghost" returns. "Save this as accounting" hires the agent and carries this session's turns into it. A transfer cancels any change waiting for a yes.
5. **Hold and hang up.** "Hang on, Ghost" puts the session on hold: the gate after STT drops everything the mic hears, unlogged, until an utterance that starts by addressing Ghost resumes it with the rest as the next turn; a hold ends quietly after 30 minutes (ADR 0021). A goodbye ("talk to you later", "bye", "that's all") or the idle timeout (45 s default) ends the pipeline, restores each music stream to its exact previous volume, and returns to passive. Every spoken turn was persisted as it happened; the next wake is `ghost` again.

### What a department reaches (ADR 0024, ADR 0034)

Every department reaches every connected app; its `permissions` narrow that for its own
sessions: a `block` rule takes a tool away or refuses the calls it matches
(`github.*(repo!=site)` keeps a department to one repository), an `ask` rule makes a change
wait for a yes, a `quiet` one keeps it unmentioned. A department can never `allow` what the
operator's file does not. A retired agent leaves voice routing and the model's list, keeps
its transcript, memory and audit, and can be rehired. An agent is moved to another
department with its folder, memory and transcript; a running session in it follows, under
the destination's rules.

### The waiting list (ADR 0029)

What needs the operator is one list (`ghost_core.waiting`): items with stable keys in the
`items` table, from watches and from Ghost itself (a change waiting for a yes, an app that
isn't connected or wants a new sign-in, a department folder with a problem). A source syncs the whole set it
sees; a new key is news, a key that is gone is resolved. An item is delivered when a route
passed it on and acknowledged when the operator heard it; only acknowledged or snoozed stops
the alerting, and a changed item alerts again. `GET /waiting`, `POST /waiting/ack` and
`POST /waiting/snooze` serve it, `Status.waiting` counts what is still to be heard, the bus
sends a `waiting` event, and the model reads it with `waiting_list` ("what needs me?"),
typed fields only.

### Routes (ADR 0031)

`ghost_core.routes.RouteEngine` ticks with the waiting list's refresh (every 15 s): for each
item still to be heard, outside quiet hours and do-not-disturb, past the batch window, it
picks the quietest enabled route the item's urgency allows (desk routes only when the
operator is at the desk: a desk session in the last 15 minutes, or the desktop app
connected), and after `escalate_after` minutes without an acknowledgment the next one up;
a route that knows nobody heard (desk voice with no reply) hands the item on at once. A
bare wake in Ghost opens with the digest instead of a greeting ("Hi. Two things are
waiting: …"), which counts as heard. "Nothing until two" is a fixed intent (no model) that
sets do-not-disturb. The policy is `[routes]` in `policy.toml`; the model sets none of it.

Desk voice (`desk`, opt-in, ADR 0032) is the daemon's own route (`ghostd.routes.DeskRoute`):
only when the desk is idle, the passive loop takes it like a wake, releases the wake
listener, and opens a session that greets with `[routes].greeting` and nothing about the
items, which wait in the context. The phase is `asking`. A reply within 15 seconds is the
acknowledgment; "not now" snoozes them for 30 minutes; silence ends the session with
nothing kept, and the engine hands the items to the next route at once.

### Memory and compaction

Each conversation loads only its own persona, memory and notes. The memory is a rolling summary kept in `memory.md`, produced by Pipecat's in-session summarizer with a tokens-only trigger (24k estimated tokens, last 20 messages kept raw, summary up to 3k; `GHOST_COMPACT_*`). Each applied summary is recorded as a version and written to the file through the gate; the file is the source of truth on resume, so an edit by hand is folded into the next version. Summarized turns stay in SQLite, flagged, and are pruned after `GHOST_TURN_RETENTION_DAYS` (90). Raw tool results are never stored, so old untrusted text does not re-enter a later session.

## Writes

Speech-to-text runs inside our own process, so the backend observes the transcribed
user turn directly. `core` accepts a confirmation only from a transcript frame the
pipeline produced for the current session, never from LLM output. What Ghost does without
asking is the operator's call (ADR 0011, ADR 0034); by default, everything:

1. The LLM calls a change: an app's tool under its own name (`github_issue_write`), or
   `change` for one of Ghost's own operations. The source's `prepare` turns the arguments
   into a target and a typed change set: for an app, checked against the tool's own schema
   and bounded (16,000 characters a string, 100 items a list), with the words that would
   ask for it read off the tool's name and short arguments.
2. `core` checks the source's own limit, then resolves the level from the permissions for
   this source, tool and arguments: the operator's rules plus the active department's,
   the most careful match, else the mode (`open` announces).
3. `core` stores an immutable proposal. For one of Ghost's own operations the writer
   snapshots the target first; an app's change is not read before or after, and the audit
   row says so.
4. At `announce` or `quiet` the proposal is executed at once. At `ask` (`confirm`) it is
   attached to the session: the operator's own request, when it plainly asks for that
   call, is the yes (ADR 0017); otherwise the next user turn answers it, and the classifier
   (Jev when configured, else the regex allowlist, negation guard in both) must call it a
   clear yes; a no cancels it. A turn about something else, a hang-up or a transfer sets
   it aside instead (ADR 0030): it waits on the waiting list for `[defaults] pending_ttl`
   (an hour unless set), and a Confirm by id in the operator's app, or Ghost taking it
   up again (`bring_up_change`) and a yes, applies it.
5. Execution claims the proposal atomically, applies each step through the writer, and
   records success, partial, or failed. For an app, what it answered (the new issue's
   number, the sent message) comes back as the read-back, trimmed.
6. The model says what happened in a few words, or asks one short question when the
   change waits. There is no undo (ADR 0015): asked to change something back, Ghost asks
   for the opposite change like any other.

## Repository layout

```text
ghost/
├── apps/
│   ├── ghostd/          # Python daemon: wake, sessions, apps, control API server, phone listener
│   ├── desktop/         # Tauri 2 + React: tray, chat, confirmation card, apps, permissions, audit, settings
│   └── mobile/          # Expo + React Native, Android: Home, session, activity
├── packages/
│   ├── core/            # Python: tool sources, apps, catalog, sign-in, departments, gate, permissions, confirmation, audit, conversations
│   ├── voice/           # Python: Pipecat pipeline factory, Parakeet, sounddevice transport, slots
│   ├── api/             # Python: FastAPI on the Unix socket; openapi.json is the contract
│   ├── client-ts/       # TypeScript: GENERATED from packages/api/openapi.json (pnpm gen)
│   └── ui/              # TypeScript: React pieces shared by desktop and mobile
├── packaging/           # Dockerfile.build, build-debs.sh, debian/ control files and unit
├── infra/
│   ├── example/         # Ghost's seed folder: apps.json, department.json, AGENTS.md (make env)
│   ├── templates/       # three department folders to copy under ~/.config/ghost/departments/
│   ├── systemd/         # ghostd.service for a checkout + env renderer
│   └── pipewire/        # echo-cancel config
└── docs/
```

Two toolchains, one root: a `uv` workspace for Python and a `pnpm` workspace for
TypeScript. They never import each other; the only crossing is
`packages/api/openapi.json` → `packages/client-ts`, generated and checked by CI.

Python 3.12. Import direction is strictly
`ghostd -> {voice, api} -> core`; `core` imports nothing from the others, and nothing
knows an app by name: `ghostd` connects whatever `apps.json` lists. See
[apps.md](apps.md) to connect one.

### How the gate is wired into the pipeline

```text
transport.input -> [audio_in] -> Parakeet STT -> InterceptorProcessor -> ConfirmationProcessor -> [after_stt]
  -> user aggregator -> [before_llm] -> LLM -> [after_llm] -> thinking sound -> [before_tts] -> Kokoro -> [audio_out]
  -> transport.output
```

The bracketed names are the slots Ghost's own processors sit in, one
fresh instance per session. `InterceptorProcessor` offers each user turn to the
the turn interceptors first (Ghost's own fixed intents: a goodbye, "hang on",
"never mind" while a change is pending); a handled turn is spoken to or
acted on directly and never reaches the model.

`ConfirmationProcessor` sees every `TranscriptionFrame`. If the gate holds a pending
proposal for the session, the frame's text is the confirmation evidence, judged with
Ghost's last reply as context: a clear yes executes, anything else cancels. Transcripts
that arrive while Ghost is still speaking are not evidence at all: the operator has
not heard the question yet, and the text may be Ghost's own voice. The verified
outcome is appended to the context as a system note, so the model's next sentence
describes what actually happened. The model's tools are every connected app's tools (reads called directly, changes through
the gate), the agents' and the departments' read tools, `change`, `hang_up` and `hold`.
There is no change that bypasses the gate.

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
- LLM default: `claude-sonnet-5-5` with `thinking: {"type": "between_tools"}` (0.77 s to first token; see the [benchmark](benchmarks/2026-10-16-speech-cpu.md#llm-time-to-first-token)). Where a turn's time goes, measured: [2026-10-19](benchmarks/2026-10-19-latency.md).
- A dedicated LLM API key for Ghost, so it can be revoked on its own. `GHOST_LLM_ENDPOINT=openrouter` (OpenRouter's Anthropic API, `OPENROUTER_API_KEY`) is one; the default, `proxy`, is whatever `ANTHROPIC_BASE_URL` points at.

## Shipping

Three Debian packages, built inside `packaging/Dockerfile.build` so the host needs no
Tauri libraries: `heka-ghostd` (a bundled Python 3.12 and venv under `/opt/ghost`, the
`ghostd` shim, a per-user systemd unit, the PipeWire templates), `heka-ghost-desktop`
(the Tauri app, from `tauri build --bundles deb`), and the `heka-ghost` meta-package.
`make build-image`, `make deb`, `make deb-test`. Speech models are not in the package;
they download on first run. Secrets are never in the package: the desktop app writes
`~/.config/ghost/env` (0600), or the operator's vault renders it when present. Target
is Ubuntu 26.04 amd64.
