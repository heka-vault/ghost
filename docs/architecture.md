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
    LLM --> SK[MCP servers<br/>github, mobbin, ...]
    SK --> DOM[core<br/>gate, policy, audit]
    DOM --> DB[(SQLite WAL)]
  end
  LLM <-->|HTTPS| PROXY[Anthropic-compatible endpoint<br/>Claude]
  DOM -->|fine-grained PAT| GH[GitHub REST]
  SK -->|reads| GH
```

The phone app (`apps/mobile`, Expo, Android) is a second transport into the same pipeline (ADR 0005, ADR 0013), so the gate, the policy and Jev apply unchanged. It never uses the control API. A question from Ghost is answered by voice on the phone: there is no Confirm button there. Its screens read a small `Desk` interface (`apps/mobile/src/desk/types.ts`). With the screen off a session keeps the mic through a microphone foreground service (`apps/mobile/modules/ghost-call`, a local Expo module) with an End action in its notification, and routes audio to a car or headset when one is connected. It reaches the house through the phone listener below (ADR 0016): on the LAN directly, and from anywhere else through the operator's own WireGuard network, where the desk and the phone are peers of a hub that forwards only between the two (ADR 0020). Nothing public is added: no STUN, no TURN, no relay; the tunnel is the operator's.

### MCP servers

Each department declares its MCP servers in its folder (ADR 0022, ADR 0024): `mcp.json` in the shape other clients share (a streamable HTTP `url` with `headers`, or a stdio `command` with `args` and `env`; `${NAME}` in either is a secret from the environment, filled at connect time and never logged; `use` reaches one of Ghost's servers instead), and `department.json` with what the gate may do with each (`reads`, `scope`, `writes.<op>`, `levels`, `targets`). `ghostd` connects Ghost's at start and a department's on the first transfer into it (`ghost_core.mcp`, `ghost_core.departments.Fleet`), reads each server's tools, and offers to the model the ones the server marks read-only or the folder lists under `reads`, as ordinary typed read tools under the server's name. A write is offered only once declared (the tool, pinned arguments, the target arguments, the read that shows the target, the words that ask for it, risk and bounds); then it is a gate operation like any other, with its level from the folder, the operator's plain request as the yes, snapshot, stale check, read-back and audit. A `scope` declaration says which arguments name the scoped item and lists the allowlist, which is also what the department owns. A server that fails to connect is logged, reported as its department's problem and skipped; `ghostd check`, `GET /mcp`, `GET /departments` and the desktop's Servers and Departments screens show each server's state, reads, writes and levels. Everything a server sends is data: the model sees the first sentence of each description, results are trimmed, and nothing in them changes a level or a scope. See [mcp.md](mcp.md).

### Skills

A skill is a `SKILL.md` folder (ADR 0023, ADR 0024), read nearest first: the department's own (`skills/`, `.agents/skills/`, `.claude/skills/` under its folder or its `root`), the operator's `~/.agents/skills/`, then Ghost's own `~/.config/ghost/skills/`. `ghost_core.skills` finds them, puts an index (name and one line each) in the prompt, and offers `read_skill`, which returns a body or one of the files beside it, trimmed and inside the folder only. A `SKILL.md` that will not parse or whose name is not its folder's is a problem the operator sees and the model does not. Adding and removing Ghost's own are gate operations (`skills.add`, `skills.remove`), so the desktop's drag-and-drop and "Ghost, add the invoicing skill from this folder" are the same audited write. Skill text shapes what the model does, never what it may do; nothing in a skill is run. See [skills.md](skills.md).

### Departments and agents

A department is a folder (ADR 0024): `~/.config/ghost/departments/<slug>/` with `AGENTS.md` (its rules, in every agent's prompt), `mcp.json`, `department.json` (title, aliases, voice, an optional `root` whose files are read in place, and the declarations), a shared `memory.md`, `skills/`, `docs/` and `agents/`; Ghost is the config folder itself (`ghost_core.departments`). An agent is a conversation (ADR 0014) hired into a department (`conversations.create`), moved between them with its folder, memory and transcript (`conversations.move`; a running session follows), retired and rehired. A session in a department is built from what its folder gives (`Fleet.sources_for`, `Skills.source`, the department's `read_department_doc` and the agents' tools): every read tool is registered once and dispatched to the active agent's reach, and a transfer swaps the prompt, the voice and the advertised tools. Ghost writes into a folder only through the gate: an agent's files, the shared memory (`departments.remember`) and a rule appended verbatim to `AGENTS.md` (`departments.add_rule`, `ghost_core.department_tools`); never `mcp.json` or `department.json`. `infra/templates/` ships three folders to copy; `ghost-scripts` runs a skill's scripts as a gated, skill-scoped server. See [departments.md](departments.md).

### The phone listener

`ghostd` serves a second, narrow HTTP app for the phone when `GHOST_PHONE_LISTEN` is set (one or more `host:port`, comma separated, bound to exactly those addresses: the LAN one first, then the tunnel one; off when empty). The first must bind at start; a later one whose interface isn't up yet is retried every few seconds. `/pair` and `/hello` carry the whole list, so the phone keeps every address and uses the first that answers. It is not the control API, shares no routes with it, and doesn't proxy it (`ghostd.phone`). Six routes: `POST /pair` spends a six-digit code the desktop app got from the control API's `POST /phone/pairing`; `GET /hello`, `GET /audit` and `GET /conversations` return the control API's `Status`, `AuditPage` and the live threads, built by the same functions (`ghost_api.status_snapshot`, `ghost_api.audit_page`, `ghost_api.conversations.conversation_view`); `POST`/`PATCH /offer` is WebRTC signalling, and the offer may name the thread the session opens in (the phone's Home lists them under the ghost; a thread called directly greets as itself); `POST /end` ends the phone's session. Every route but `/pair` is HMAC-signed both ways by a middleware, with a 60 s clock window and a nonce cache; the paired device and its secret are in `~/.local/share/ghost/phone.json` (0600). The wire format is [phone-protocol.md](phone-protocol.md).

An accepted offer becomes a `SmallWebRTCConnection` with no ICE servers, which the listener hands to the daemon. The passive loop waits on it next to the wake detector: it cancels the wake task, which releases the desk mic, and runs a session whose transport is a `SmallWebRTCTransport` (16 kHz in, Kokoro's 24 kHz out, the thinking sound mixed into the phone's track). A phone session doesn't duck music or chime on the desk, and `Status.session_origin` is `phone`. An offer while any session runs, or while the models load, is refused (409 `busy`, 503 `not_ready`). The session ends when the peer closes or stops its keepalive, on `POST /end`, or on RTVI `disconnect-bot`, and its ending closes the peer. Phone sessions are ended with an EndFrame, not cancelled: Pipecat leaves a processor that saw a CancelFrame refusing frames, and Kokoro's processor is shared by every session.

The data channel carries RTVI through `ghost_voice.rtvi.GhostRTVIProcessor`, which acts only on `client-ready` and `disconnect-bot` (no typed text, raw audio or function results), with an observer that sends `bot-ready` and the speaking events but no transcripts, LLM text, metrics or logs. The daemon forwards its own event bus (`status`, `turn`, `pending`, `outcome`, `conversation`; never `log`) to the phone as `server-message`s, the same JSON the control API's WebSocket sends.

## Components

| Component | Responsibility |
|---|---|
| `ghostd` | Long-running desktop daemon. Owns the microphone, runs the passive wake listener (local VAD and Parakeet matching a phrase, ADR 0010), and on wake builds a Pipecat pipeline with a local audio transport. Ducks and restores music, plays the chime and the thinking sound, and returns to passive on a goodbye or the idle timeout; "hang on, Ghost" holds the session until it is addressed by name again (ADR 0021) |
| `voice` | Builds the pipeline: VAD and turn detection, STT, the LLM service, and TTS behind one config, so models can be swapped (ADR 0004) |
| `api` | The control API (ADR 0013): FastAPI on a user-owned Unix socket. Status and a WebSocket of events, wake and sleep, typed turns, the pending confirm-level change with Confirm and Cancel, audit, abilities, the policy file, write-only secrets, devices and setup. `packages/api/openapi.json` is generated from it and `packages/client-ts` from that. Nothing passive crosses it |
| `core` | The write gate and what feeds it (ADR 0007, 0011, 0022, 0024): tool sources (`ghost_core.source.ToolSource`), the MCP client with the write declaration (`ghost_core.mcp`), the department folders and their servers' lifecycle (`ghost_core.departments`, `ghost_core.department_tools`), the operator's autonomy policy, proposals over typed targets and change sets, the confirmation classifier, the audit store, and the agents (ADR 0014). Imports nothing from the other packages. |
| `core/conversations` | The agents (ADR 0014, ADR 0024): the registry mirrored from the folders, the transcript, every compaction version, and the files per agent under its department's `agents/<slug>/` (`agent.json`, `persona.md`, `memory.md`, `notes.md`). Their mutations are gate operations under `[conversations]` |
| SQLite | Proposals, operations, operation events, the conversation registry, session transcripts, compaction versions |

## Conversation flow

1. **Passive.** `ghostd` runs a short local pipeline: VAD, Parakeet, phrase matcher. A transcript without a wake phrase is dropped where it is produced. Nothing is stored or sent.
2. **Wake.** "Hey Ghost", "Ghost, you there?" or another configured phrase (`GHOST_WAKE_PHRASES`) matches. With `GHOST_WAKE_MODE=onnx` the trained detectors listen instead (ADR 0025, `ghost_voice.wakeword`): "hey ghost", and, when its model is installed, Ghost addressed by name with a request ("Ghost, what is the status of …"). A model fires as its phrase ends, Parakeet checks the hit (`GHOST_WAKE_VERIFY`), and the chime plays then, while the rest of the sentence is still being heard. `ghostd` plays a chime, ducks the configured music streams, and starts the pipeline. Words spoken after the phrase in the same breath become the first turn, so "hey ghost, what needs me?" skips the greeting. `make wake` still triggers it by hand.
3. **Dialogue.** Silero VAD and Smart Turn decide when you have finished speaking; Ghost waits a short grace in case you go on, and the model starts answering during it, its answer held until the turn is confirmed and dropped if you keep talking (`GHOST_TURN_SPECULATE`; a held answer that reaches a tool call is withdrawn and the confirmed turn makes the call). Parakeet transcribes, the LLM answers or calls skill tools, and Kokoro speaks a clause at a time, so the voice starts after the first few words (`GHOST_TTS_CLAUSES`). Barge-in stops playback once the STT has heard at least two words from you (`GHOST_BARGE_IN_MIN_WORDS`); VAD alone never interrupts, so a leak of Ghost's own voice past the echo canceller cannot cut it off. While Ghost speaks, Parakeet re-reads your unfinished speech every 0.3 s (`GHOST_BARGE_IN_INTERIM_SECS`), so you needn't pause first; those interim words only ever decide barge-in, and the turn, the transcript and the model get the final transcript. Every tool call in a reply runs, not only the last (Pipecat's Anthropic service keeps one); and the model's output cap (`GHOST_LLM_MAX_TOKENS`, 2000 tokens) counts tool calls too, so a reply that hits it is redone in smaller steps rather than losing them (`ghost_voice.llm`).
   A typed turn from the desktop app (`POST /session/say`) enters the same pipeline after
   the STT stage, so it meets the same tools and the same gate as speech.
4. **Transfer.** Every session opens in `ghost`. "Transfer me to accounting" (a department's name lands on its default agent) is an LLM tool: Ghost's voice announces it, a short tone plays, the TTS voice and the system prompt change (Ghost's rules, the department's rules and shared memory, the agent's persona and memory), the tools become what the department reaches, its own servers start if they are not up, and the context is replaced with that agent's rolling summary, pinned notes and recent turns; the agent greets with a one-line recap. "Back to Ghost" returns. "Save this as accounting" hires the agent and carries this session's turns into it. A transfer cancels any change waiting for a yes.
5. **Hold and hang up.** "Hang on, Ghost" puts the session on hold: the gate after STT drops everything the mic hears, unlogged, until an utterance that starts by addressing Ghost resumes it with the rest as the next turn; a hold ends quietly after 30 minutes (ADR 0021). A goodbye ("talk to you later", "bye", "that's all") or the idle timeout (45 s default) ends the pipeline, restores each music stream to its exact previous volume, and returns to passive. Every spoken turn was persisted as it happened; the next wake is `ghost` again.

### Departments own items (ADR 0024)

A department's server declaration carries an allowlist (`scope.items`), and those items
are what the department **owns**. On every call the pipeline hands the server the active
agent's reach: read tools list only what the department covers, a department with one item
stops asking for it ("close seventeen"), and the server's prompt paragraph names only the
department's items. The gate refuses a write whose target is outside the department's
allowlist before policy or autonomy are consulted. Ghost reaches its own servers and
allowlists.

A request about an item another department owns gets a tool result naming the owner, and
Ghost offers to transfer; with `GHOST_DEPARTMENT_ROUTING=1` (off by default) it transfers
and hands the request over. A retired agent (announce-level) leaves voice routing and the
model's list, keeps its transcript, memory and audit, and can be rehired. An agent is
moved to another department with its folder, memory and transcript; a running session in
it follows.

### The waiting list (ADR 0029)

What needs the operator is one list (`ghost_core.waiting`): items with stable keys in the
`items` table, from watches and from Ghost itself (a change waiting for a yes, a server of
Ghost's that is down, a department folder with a problem). A source syncs the whole set it
sees; a new key is news, a key that is gone is resolved. An item is delivered when a route
passed it on and acknowledged when the operator heard it; only acknowledged or snoozed stops
the alerting, and a changed item alerts again. `GET /waiting`, `POST /waiting/ack` and
`POST /waiting/snooze` serve it, `Status.waiting` counts what is still to be heard, the bus
sends a `waiting` event, and the model reads it with `waiting_list` ("what needs me?"),
typed fields only.

### Memory and compaction

Each conversation loads only its own persona, memory and notes. The memory is a rolling summary kept in `memory.md`, produced by Pipecat's in-session summarizer with a tokens-only trigger (24k estimated tokens, last 20 messages kept raw, summary up to 3k; `GHOST_COMPACT_*`). Each applied summary is recorded as a version and written to the file through the gate; the file is the source of truth on resume, so an edit by hand is folded into the next version. Summarized turns stay in SQLite, flagged, and are pruned after `GHOST_TURN_RETENTION_DAYS` (90). Raw tool results are never stored, so old untrusted text does not re-enter a later session.

## Writes

Speech-to-text runs inside our own process, so the backend observes the transcribed
user turn directly. `core` accepts a confirmation only from a transcript frame the
pipeline produced for the current session, never from LLM output. How much Ghost does
without asking is the operator's call (ADR 0011):

1. The LLM calls `change` on a source's operation (a server's declared write, or a line operation): the operation's `prepare` turns the
   arguments into a target and a typed change set.
2. `core` checks the source's own allowlist, then resolves the autonomy level for
   this source, operation, and target from `~/.config/ghost/policy.toml` (`deny`,
   `confirm`, `announce`, `quiet`; a missing entry falls back to the operation's
   risk class).
3. `core` fetches the target through the source's writer (for a server, the declared read) and stores an immutable
   proposal with that snapshot.
4. At `announce` or `quiet` the proposal is executed at once. At `confirm` it is
   attached to the session: the next user turn answers it, and the classifier (Jev when
   configured, else the regex allowlist, negation guard in both) must call it a clear
   yes; a no cancels it. A turn about something else, a hang-up or a transfer sets it
   aside instead (ADR 0030): it waits on the waiting list for `[defaults] pending_ttl`
   (an hour unless set), and a Confirm by id in the operator's app, or Ghost taking it
   up again (`bring_up_change`) and a yes, applies it. Either way the target must not
   have changed since the snapshot.
5. Execution claims the proposal atomically, applies each step through the writer, reads
   the target back, and records success, partial, or failed. Anything created carries a
   hidden idempotency marker so a retry never duplicates it.
6. The model gets the verified result and says what happened in a few words, or asks
   one short question when confirmation is needed. There is no undo (ADR 0015): asked to
   change something back, Ghost proposes the opposite change like any other.

## Repository layout

```text
ghost/
├── apps/
│   ├── ghostd/          # Python daemon: wake, sessions, server connections, control API server, phone listener
│   ├── desktop/         # Tauri 2 + React: tray, chat, confirmation card, policy, audit, settings
│   └── mobile/          # Expo + React Native, Android: Home, session, activity
├── packages/
│   ├── core/            # Python: tool sources, MCP client and declaration, departments, gate, policy, confirmation, audit, conversations
│   ├── voice/           # Python: Pipecat pipeline factory, Parakeet, sounddevice transport, slots
│   ├── api/             # Python: FastAPI on the Unix socket; openapi.json is the contract
│   ├── client-ts/       # TypeScript: GENERATED from packages/api/openapi.json (pnpm gen)
│   └── ui/              # TypeScript: React pieces shared by desktop and mobile
├── packaging/           # Dockerfile.build, build-debs.sh, debian/ control files and unit
├── infra/
│   ├── example/         # Ghost's seed folder: mcp.json, department.json, AGENTS.md (make env)
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
knows a server by name: `ghostd` connects to whatever the department folders declare. See
[mcp.md](mcp.md) to declare one.

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
describes what actually happened. The model's tools are the servers', the agents' and the departments' read tools,
`change`, `hang_up` and `hold`. There is no tool that bypasses the gate.

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
