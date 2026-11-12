# Ghost architecture

**Status:** first increment under way, 2026-10-16; the daemon rewritten in Rust and shipped inside the desktop app, 2026-10-29 (ADR 0038, ADR 0039). This replaces the deployment and stack parts of [original/technical-specification.md](original/technical-specification.md). The product behavior in [original/prd.md](original/prd.md) still applies unless an ADR says otherwise.

## Shape

One operator, one Linux workstation (ADR 0001). Ghost is one desktop app with the daemon inside (ADR 0038): the app runs `ghostd` while the operator is logged in, lives in the tray, and Quit stops everything; the `ghostd` user unit is the headless mode for a desk with no display. The daemon and the app's shell are one Cargo workspace (ADR 0039). Only two kinds of thing leave the machine: LLM calls, and calls to the apps the operator connected (ADR 0034).

```mermaid
flowchart LR
  subgraph WS[Linux workstation]
    MIC[Mic via PipeWire<br/>echo-cancelled source] --> WAKE[Wake phrase<br/>local VAD + Parakeet, "hey ghost"]
    WAKE -->|wake| PIPE[Ghost's pipeline<br/>crates/ghost-voice]
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
  LLM <-->|HTTPS| PROXY[The model connection<br/>Anthropic or OpenAI-compatible API]
  SK -->|signed in, or a key| EXT[GitHub, Linear, ...]
```

The phone app (`apps/mobile`, Expo, Android) is a second transport into the same pipeline (ADR 0005, ADR 0013), so the gate, the policy and Jev apply unchanged. It never uses the control API. In a call, a question from Ghost is answered by voice. Outside a call the phone holds a signed live stream and answers questions, and confirms or cancels a change waiting for a yes, with signed requests (ADR 0051; [phone-protocol.md](phone-protocol.md)). The stream is read natively, in Kotlin, by a foreground service of type `connectedDevice` (`apps/mobile/modules/ghost-call/android/.../link/`), so it lives with the app's screen closed and its JavaScript not running: it keeps the list and the cursor (encrypted with a Keystore key), posts a notification per item when the desk's phone route alerts (Got it and Later; a question's choices and an inline answer; Yes and No for a change), sends `seen` only once a notification is really up, raises a full-screen incoming call for a ring, and sends every action signed with its `id` until the desk signs an answer, taking a notification down only on the stream's word. It never holds the microphone, has its own quiet notification, can be turned off in Settings, and starts at boot only when the operator says so. Home lists what is waiting from the same state. Its screens read a small `Desk` interface (`apps/mobile/src/desk/types.ts`). With the screen off a session keeps the mic through a separate microphone foreground service (`apps/mobile/modules/ghost-call`, a local Expo module) with an End action in its notification, and routes audio to a car or headset when one is connected; during a session the connection keeps running, posts silently and shows no ring. It reaches the house through the phone listener below (ADR 0016): on the LAN directly, and from anywhere else through the operator's own WireGuard network, where the desk and the phone are peers of a hub that forwards only between the two (ADR 0020). Nothing public is added: no STUN, no TURN, no relay; the tunnel is the operator's.

### The app and the daemon

`tauri build` produces one .deb and one AppImage carrying the desktop app (`apps/desktop`, Tauri 2 and React) and `ghostd`, with `ghost-mock-github`, `ghost-scripts` and the ONNX Runtime the models were measured on beside it. The shell (`apps/desktop/src-tauri`) supervises the daemon: at launch it adopts one already answering on the control socket (`make dev`'s, or one started by hand), else runs `ghostd restore-volumes`, spawns `ghostd run` in its own process group with `settings.env`, pipes its output through `systemd-cat -t ghostd` so `journalctl --user -t ghostd` keeps working, and restarts it with backoff when it exits. Quit sends TERM to the group, waits up to 20 s, then KILL, then `restore-volumes`; the daemon also exits on its own when its parent is gone. The same binary runs under the user unit by itself; nothing in it knows which way it was started beyond `GHOST_LAUNCHER`, which only changes what the desktop says about restarting ([releasing.md](releasing.md)).

### Apps

The operator connects an app to Ghost once and every agent reaches it (ADR 0034).
`~/.config/ghost/apps.json` lists them: a URL Ghost signs in to (OAuth, `ghost_core::signin`,
tokens in a 0600 file per app, the browser back on a loopback address only while a sign-in
is open) or sends a password to, or a command it runs on this computer; passwords are
`${NAME}`, filled from Ghost's vault when the app connects (ADR 0036). The desktop's Apps page (Settings › Apps) connects them from Ghost's own copy of the public app
registry (`ghost_core::catalog`: the first-party apps first, then the rest in the
background, refreshed at most hourly), by address, or from another tool's pasted config.
`ghostd` connects them at start (`ghost_core::apps::Apps`) and one connected later joins
the gate at once and an open session's tools from its next model call. Every tool is
offered under the app's name: one the app marks read-only is a read, called directly;
every other tool is a change, which goes to the gate. An app that fails to connect, or
wants a new sign-in, is on the waiting list and the Apps page and is skipped. Everything
an app sends is data: the model reads each description up to 1,024 characters and the
app's own instructions up to 2,000, one result up to 24,000 characters with a note when it
was cut, all of it redacted first, and nothing in them changes a permission.

The client keeps up with the app (ADR 0046). Its tools are listed again when it says they
changed, and the model's stage asks for the tool list before every model call, replacing
it only when it differs. A call may take 300 seconds and stops when the turn it was made
for is cancelled, the operator hangs up or its time is up; the app is sent
`notifications/cancelled` and the model reads an error (`ghost_core::apps::with_cancel`,
`TurnControl::stop_calls`). A connection that died is opened again by the next call and
in the background with backoff (`Apps::supervise`). An app that advertises documents or
prompts gets reads over them (`<app>_list_resources`, `<app>_read_resource`,
`<app>_list_prompts`, `<app>_get_prompt`). An app's question for the operator (a form of
one field) becomes a question from the app on the waiting list, held until the app's own
limit and declined then (`ghost_core::questions::AppAsker`, ADR 0051). Past 40 tools or 30,000 bytes of schema, most tools are
found with `ghost_tools_search` and not offered by name. See [apps.md](apps.md).

### Ghost's own app

Ghost is an app too, and its own agents use it (ADR 0045). `ghost_core::ghost_app` is the
other side of the protocol the apps speak, one handler over the daemon's shared state,
and `ghostd` serves it to itself in process: a
`tokio::io::duplex` pair, the server on one end and the ordinary app client on the other,
joined to the apps as `ghost` (`App::attach`, `Apps::attach`) before any session can open.
It needs no vault, network or sign-in, is not in `apps.json` (the name is reserved), is
never an app that "isn't connected", and is started again if its task stops. Its tools are
offered as `ghost_<tool>` and take the same path as any app's: a read is called directly,
a change goes through the gate, and the operator's `ghost.*` rules apply.

| Tool | Kind | |
|---|---|---|
| `ghost_docs_search`, `ghost_docs_read` | read | Ghost's own documentation, built into the binary (the README, the glossary, this page, `apps.md`, `connect-ghost.md`, `tools.md`, `skills.md`, `folder.md`, `performance.md`, `phone-protocol.md`), split by heading; no model, no network |
| `ghost_status` | read | Version, phase, the session and its agent, the apps and their state, how much is waiting, the model connection's provider and model, Ghost's own problems; built from the control API's `Status` |
| `ghost_audit_recent` | read | Recent changes from the audit: what, which agent, when, how it ended |
| `ghost_apps_list` | read | The connected apps, their state, and how many reads and changes each offers |
| `ghost_notify`, `ghost_resolve` | change | An item from the calling agent on the waiting list, under `agent:<slug>`, and its clearing; an agent clears only its own |
| `ghost_waiting_ack`, `ghost_waiting_snooze` | change | What `POST /waiting/ack` and `/waiting/snooze` do |
| `ghost_tools_search` | read | The connected apps' tools, and Ghost's own past its core, that match a few words, with the name to call each by (ADR 0046, ADR 0052) |
| `ghost_schedules_list` | read | The schedules: each one's agent, when it runs next and how the last run ended; with an id or a name, its instruction and what its last runs said (ADR 0047) |
| `ghost_schedule_manage` | change | Create, update, pause, resume, remove or run a schedule now. When is one of `at`, `every` or `cron`. An agent manages its own; Ghost manages all |
| `ghost_ask` | change | A question for the operator on the waiting list, up to four choices, returning its key (ADR 0051). In a session it does not wait; from a run it may wait up to two minutes, and otherwise its answer starts one more run of the same agent |
| `ghost_question`, `ghost_questions_list` | read | A question the caller asked and its answer; the open ones and the last day's. Ghost reads every asker's |
| `ghost_question_cancel` | change | Take back an open question the caller asked |
| `ghost_answer` | change | The operator's answer to a question, in their exact words: only in a live session, and only when the session's dispatcher found the words in the operator's last turn (`_heard`). Never in a run, never for an outside tool |
| `ghost_recall`, `ghost_transcript_read` | read | The stored turns, searched with SQLite FTS5 (`ghost_core::recall`, an index over `turns` kept in step by triggers, ADR 0041) or paged back through: the caller's own; Ghost's (agent zero) any agent's or all; an outside client's only those of runs it started (ADR 0052) |
| `ghost_memory_read` | read | An agent's memory, notes and persona; its own, or any for Ghost |
| `ghost_apps_catalog_search`, `ghost_app_tools` | read | Ghost's copy of the catalog, never queried live on a turn; one connected app's tools |
| `ghost_app_connect`, `ghost_app_disconnect` | change | The desktop's connect and disconnect (`catalog::connect`, `apps::address_spec`, `apps::edit_apps`, `Apps::add`). They ask by default. A sign-in or a key is the operator's: the app is left with an item on the waiting list saying what to do |
| `ghost_watches_list`, `ghost_watch_manage` | read, change | The watches, and one validated writer of `watches.json`: a polled watch only, recorded as `made_by` the agent, refused whole when the file would not pass `ghostd check`. Making or changing one asks by default |
| `ghost_routes_read`, `ghost_quiet_until`, `ghost_quiet_off` | read, change | The routes and do-not-disturb, as "nothing until two" sets it |
| `ghost_permissions_read`, `ghost_settings_read` | read | The mode and the rules; the settings that are not passwords or addresses, and the model in use. No tool changes either |
| `ghost_output_read`, `ghost_output_delete` | read, change | A file in `out/`, a part at a time within the result budget, and its deletion |
| `ghost_ask_agent` | change | A written task for another agent, through the run with nobody there (ADR 0047), marked `[asked by <agent>, another agent]` under `run-agentask-<id>`; depth 1, three running per session |
| `ghost_skill_write` | change | A `SKILL.md` in Ghost's own skills folder, the one file and nothing else; asks by default |

Three things differ from an app the operator connected. Its text is Ghost's own, so
descriptions reach the model whole and results are not cut on the way out (the tools cap
themselves: a section at 8,000 characters, said when cut; the tool search at an app's
result budget, since what it returns is the apps' text). The calling agent is never an argument the
model writes: the session's dispatcher sets `_agent` to the agent the session is with.
And where no rule names a `ghost.*` change and the mode is `open`, it runs at `quiet`,
since an item on the waiting list is its own announcement; `ask = ["ghost.schedule_manage"]`
has Ghost ask before an agent sets up a schedule. Four kinds of change ask by default
instead (ADR 0052, `ghost_app::asks_by_default`): connecting or disconnecting an app,
writing a skill, and making or changing a watch. The handler keeps nothing per
connection, and the same one is served to the operator's other tools
([below](#the-listener-for-outside-tools)), less what `ghost_app::NOT_FOR_OUTSIDE` names.

Only a core of these tools is offered by name on every turn (`ghost_app::CORE`: `notify`,
`resolve`, `ask`, `answer`, `waiting_ack`, `status`, `docs_search`, `recall`,
`schedule_manage`, `tools_search`); the rest are found with `ghost_tools_search` and
offered by name from the next model call, through the same deferral ADR 0046 made for
connected apps (`Apps::hidden`, `Recent`). Measured by
`what_ghosts_own_app_costs_every_turn`: 10 tools and 7,051 bytes of schema on every turn,
where before ADR 0052 it was 16 tools and 8,397 bytes; all 35 offered by name would be
17,169. `ghost_docs_search` returns a section under 1,200 characters whole and a longer
one as its best whole sentences or list items (400 characters at most), so an answer
rarely needs `ghost_docs_read` after it. [tools.md](tools.md) lists every tool with its default level.

### Permissions

Everything runs unless the operator says otherwise (ADR 0034). `[permissions]` in
`policy.toml` holds `block`, `ask`, `quiet` and `allow` rules over an app, a tool, or a
tool's arguments (`github.issue_write(state=closed)`, `github.*(repo!=site)`), and the
`mode` for what no rule names (`open`, `ask`, `read-only`). The most careful match wins.
There is one set: nothing narrows it per agent (ADR 0035, `WriteGate::policy_for`). A tool an
unconditional `block` names is not offered; a rule with argument patterns refuses the
calls it matches. Ghost's own operations (`conversations.*`, `skills.*`, `rules.add`,
`outputs.*`) take the same rules. The desktop edits them on Settings › Permissions and per
tool on an app's page; only the `[permissions]` table of the file is rewritten
(`ghost_core::policy::write_permissions`).

### Skills

A skill is a `SKILL.md` folder (ADR 0023, ADR 0035), read from Ghost's own `~/.config/ghost/skills/`, then the operator's `~/.agents/skills/`; the first folder with a name wins. `ghost_core::skills` finds them, puts an index (name and one line each) in the prompt, and offers `read_skill`, which returns a body or one of the files beside it, trimmed and inside the folder only. A `SKILL.md` that will not parse or whose name is not its folder's is a problem the operator sees and the model does not. Adding and removing Ghost's own are gate operations (`skills.add`, `skills.remove`), so the desktop's drag-and-drop and "Ghost, add the invoicing skill from this folder" are the same audited write. Skill text shapes what the model does, never what it may do; nothing in a skill is run. See [skills.md](skills.md).

### Ghost's folder and its agents

Everything the operator edits is in `~/.config/ghost/` (ADR 0035; there are no departments), read by `ghost_core::folder` (`GhostFolder`): `AGENTS.md` (the rules, in every agent's prompt), `apps.json`, `watches.json` and `hooks.json` (with fixtures in `hooks/<name>/`), `skills/`, `out/` and `agents/<slug>/`. Files from before ADR 0035 (a `department.json` at the root, a root `memory.md` or `docs/`, anything under `departments/`) are not read; `ghostd check` and the waiting list name them. An agent is a conversation (ADR 0014) with its own name, job, persona, voice and private memory, hired (`conversations.create`), retired and rehired; Ghost is agent zero. Every agent reaches every connected app less what the permissions block, and Ghost's own tools; a transfer swaps the prompt (the rules, the agent's persona and memory) and the voice. Ghost writes into the folder only through the gate: an agent's files, a file in `out/` (`ghost_core::outputs`), and a rule appended verbatim to `AGENTS.md` (`rules.add`, `ghost_core::rules`); never `apps.json`, `watches.json` or `hooks.json`. `ghost-scripts` runs a skill's scripts as an app whose `run` is a change through the gate. See [folder.md](folder.md).

### The phone listener

`ghostd` serves a second, narrow HTTP app for the phone when `GHOST_PHONE_LISTEN` is set (one or more `host:port`, comma separated, bound to exactly those addresses: the LAN one first, then the tunnel one; off when empty). The first must bind at start; a later one whose interface isn't up yet is retried every few seconds. `/pair` and `/hello` carry the whole list, so the phone keeps every address and uses the first that answers. It is not the control API, shares no routes with it, and doesn't proxy it (`ghostd::phone`). Six routes: `POST /pair` spends a six-digit code the desktop app got from the control API's `POST /phone/pairing`; `GET /hello`, `GET /audit` and `GET /conversations` return the control API's `Status`, `AuditPage` and the live threads, built by the same functions; `POST`/`PATCH /offer` is WebRTC signalling, and the offer may name the thread the session opens in (the phone's Home lists them under the ghost; a thread called directly greets as itself); `POST /end` ends the phone's session; `/stream`, `/questions`, `/changes/*`, `/seen` and `/ring` are the live channel (ADR 0051, below). Every route but `/pair` is HMAC-signed both ways by a middleware (the stream's messages each signed on their own), with a 60 s clock window and a nonce cache; the paired device and its secret are in `~/.local/share/ghost/phone.json` (0600). The wire format is [phone-protocol.md](phone-protocol.md).

An accepted offer is answered by the media crate (`crates/ghost-rtc`, str0m by default, measured against webrtc-rs and libwebrtc; no ICE servers, the socket bound on the listener's own address), which the listener hands to the daemon. The passive loop waits on it next to the wake detector: it cancels the wake task, which releases the desk mic, and runs a session whose transport is the phone's media (16 kHz in, Kokoro's 24 kHz out, the thinking sound mixed into the phone's track). A phone session doesn't duck music or chime on the desk, and `Status.session_origin` is `phone`. An offer while any session runs, or while the models load, is refused (409 `busy`, 503 `not_ready`). The session ends when the peer closes or stops its keepalive, on `POST /end`, or on RTVI `disconnect-bot`, and its ending closes the peer. A phone session is ended, not cancelled: the end is queued behind whatever is being said, and the shared TTS stage is left as it was for the next session.

The data channel carries RTVI (`ghost_rtc::rtvi`, `ghost_voice::rtvi`): the desk acts only on `client-ready` and `disconnect-bot` (no typed text, raw audio or function results), and sends `bot-ready` and the speaking events but no transcripts, LLM text, metrics or logs. The daemon forwards its own event bus (`status`, `turn`, `pending`, `outcome`, `conversation`, `waiting`, `apps`; never `log`) to the phone as `server-message`s, the same JSON the control API's WebSocket sends.

Outside a call the phone holds one live stream (ADR 0051, `ghostd::phone::stream`): `GET /stream`, Server-Sent Events, the request signed like any other and each message signed over its nonce. The desk's `PhoneFeed` keeps a numbered log of what the phone is told (the waiting list's changes, found by comparing the list after each `waiting`, `pending` or `outcome` on the bus; every question as it is asked and settled; alerts; rings) and its last 512 entries, so a phone that reconnects with its cursor gets what it missed, and one that cannot gets a snapshot. A heartbeat goes out every 15 seconds. The phone answers with signed `POST`s, each done once per client `id`: `/questions/answer` and `/questions/decline` (route `phone`), `/changes/confirm` and `/changes/cancel` (the gate's confirm by id with `classifier: phone`), `/seen` and `/ring`. A question answered by voice reaches the phone by its choice only; the stream never carries a transcript, a turn, a log line or audio.

### The listener for outside tools

`ghostd` serves Ghost's own app to the tools on this computer the operator connected
(Claude Code, Cursor, Codex; ADR 0048, [connect-ghost.md](connect-ghost.md)): a fourth
HTTP app (`ghostd::outside`) on its own addresses (`GHOST_MCP_LISTEN`, `127.0.0.1:47824`
unless set, off when empty), one path, `/mcp`, through rmcp's Streamable HTTP service,
which answers the session-based clients and the stateless ones alike. It is not the
control API and shares no route with it, the phone listener or the webhook listener. It
starts after Ghost's own app is attached; an address that will not bind is an item on the
waiting list. An address that is not loopback is used only when the operator set it, and
is logged.

Before a request reaches the server, a guard refuses any that carries an `Origin` (a
browser sends one, a tool does not: the DNS-rebinding defence, with rmcp's `Host`
allow-list behind it) and any without a client's token. A client is a name and a token
(`ghost_core::ghost_app::clients`): made by `ghostd mcp add <name>` or
`POST /clients` (Settings › Outside tools), shown once, kept as its SHA-256 in the store's
`outside_clients` table with when it was made and last used, compared in constant time.
Five wrong tokens for a name refuse it for five minutes. A removed client is refused at
its next request, since the listener reads the store each time.

The server names the caller from what the guard authenticated (`client:<name>`, in the
request's extensions), never from an argument, and does for a call with no session what
the dispatcher and the gate do in process (`ghost_core::ghost_app::outside`): each client
has a budget (60 calls a minute, 10 of them `notify` or `say`); a read is checked against
the `block` rules and logged at debug level; every other tool is a change through the
gate's `submit` as `ghost.<tool>`, so `block` refuses it, `ask` leaves it on the waiting
list for the operator's yes while the client is told "waiting for the operator", and the
audit has the row under the session `client:<name>`. A client's `notify` is an item under
`client:<name>`, its title led by "From Claude Code:", and a client resolves only its own.

Outside clients have the tools above (without the `ghost_` prefix, which is the
session's naming) less `answer` and what `ghost_app::NOT_FOR_OUTSIDE` names (ADR 0052:
`tools_search`, since a client cannot call an app's tool; an agent's memory; the
permissions and the settings; the catalog and an app's own tools; and every change to apps,
watches, quiet, outputs, skills and other agents), and four of their own, which Ghost's
agents are not offered. A client's `recall` and `transcript_read` reach only the turns of
the runs it started: its `agent_ask` turns (`run-ask-<client>-<id>`) and its schedules'
runs. The schedules are theirs through the
same gate: what a client sets up is made by `client:<name>` and runs as the agent it
names, Ghost when it names none; a client manages only its own, and of the others reads
only that they are there and when they run.

| Tool | Kind | |
|---|---|---|
| `agents_list` | read | The agents, their jobs, whether one is in a session |
| `agent_ask` | change | A written turn for an agent and its written answer, through the run with nobody there (ADR 0047: `ghostd::outside::AskRunner` over `ghost_voice::headless`, behind `ghost_app::outside::AgentRunner`). The same dispatcher and gate; a change that resolves to `ask` in it is set aside on the waiting list, since nothing a client writes is the operator's yes; 30 model calls and ten minutes at the most; kept in the agent's transcript as `[asked by <tool>, an outside tool]` under the session `run-ask-<client>-<id>`; stopped when the client cancels the request or drops the connection |
| `ask`, `question`, `questions_list`, `question_cancel` | change, read, read, change | A question for the operator from the tool (ADR 0051): `ask` returns its key at once or holds the request up to two minutes (`wait_s`); `events_wait` says when it is settled and `question` reads the answer. A client reads only its own. There is no `answer` for an outside tool |
| `say` | change | One sentence through desk voice (ADR 0032, `ghostd::outside::Desk`): only with `desk` in `[routes].enabled`, outside quiet hours and do-not-disturb, with the desk idle; always an item on the waiting list first |
| `events_wait` | read | A long poll (60 s at most) over a numbered ring the daemon fills from its own event bus through a tap (`EventBus::tap`, not counted as the desktop being open): waiting-list items added, changed, acknowledged, snoozed or resolved, a change's outcome, a session starting or ending; never a log line, a turn or audio |

The documentation and the status are also resources (`ghost://docs/<name>`,
`ghost://status`), advertised to outside clients only: in process they would come back
to the agents as two more read tools (ADR 0046) beside `ghost_docs_read` and
`ghost_status`. Nothing here reaches a password, the policy file, a transcript or
audio, or changes a permission, an app or a client.

`ghostd mcp serve --client <name>` is the bridge for a tool that can only start a
command (`ghostd::mcp`): stdin and stdout to the listener, with that client's token from
`~/.local/share/ghost/clients/<name>.token` (0600). It waits 15 s for a daemon that is not
up and says the client's hello again to one that restarted.

## Components

One Cargo workspace (ADR 0039): the crates under `crates/` and the desktop app's shell.

| Crate | Responsibility |
|---|---|
| `ghostd` | The daemon (`crates/ghostd`). Owns the microphone, runs the passive wake listener (local VAD and Parakeet matching a phrase, ADR 0010, or the trained detectors, ADR 0025), and on wake runs a session over the desk's audio; ducks and restores music, plays the chime and the thinking sound, and returns to passive on a goodbye or the idle timeout; "hang on, Ghost" holds the session until it is addressed by name again (ADR 0021). Serves the control API, the phone listener, the webhook listener and the listener for outside tools (ADR 0048), runs the watches, the routes and the schedules (ADR 0047: an agent's run with nobody there, beside any session), and is the desktop's `Controller`. `ghostd check`, `passwords`, `mcp`, `latency`, `perf`, `push`, `restore-volumes` and `api-schema` are its subcommands |
| `ghost-voice` | Ghost's own pipeline, written for one session and one transport at a time (ADR 0039; Pipecat's ideas, not its code): typed frames over bounded channels, one task per stage, a cancellation token per session and per turn. The speech-in stage (Silero VAD, Parakeet with interim re-reads), the user's turn (Smart Turn, the patient stop with the grace, the speculative answer, barge-in), the interceptors and the confirmation processor, the model stage with compaction, the narration cut and the spoken filter, Kokoro a clause at a time or ElevenLabs with failover, the echo filter, hold, the latency timeline, and the harness behind `make perf`. Also the one thing with no audio in it: `ghost_voice::headless`, an agent's run with nobody there, over the same model port and tool dispatcher (ADR 0047) |
| `ghost-llm` | The one model client (ADR 0037): the session's streamed reply, a one-shot completion and a first-token probe, for the Anthropic Messages API and for OpenAI-compatible Chat Completions. Every tool call of a reply is delivered; a reply cut at the output cap is redone once in smaller steps and flagged `cut_off` |
| `ghost-infer` | The local models on ONNX Runtime (ADR 0004): Parakeet TDT v3 for speech to text, Silero for voice activity, Smart Turn v3 for the end of a turn, the trained wake detectors with the transcript matcher behind them, and Kokoro for speech. The runtime is loaded at start (ONNX Runtime 1.24.4, the one the parity fixtures were recorded on: in the bundle, or `~/.cache/ghost/onnxruntime/`, or `ORT_DYLIB_PATH`), and the four small model files are found the same way (`GHOST_MODELS_DIR`, the bundle, `~/.cache/ghost/models/`, where `scripts/models.sh` puts them) |
| `ghost-audio` | The audio engine: PipeWire through the project's own binding (one clock domain), a realtime-safe ring between its thread and tokio, ducking, the chime and the thinking sound |
| `ghost-rtc` | The phone's WebRTC audio behind one `Media` trait (ADR 0016, ADR 0020): str0m by default, Opus through the system libopus, the resampling either side of the codec, the RTVI subset |
| `ghost-api` | The control API (ADR 0013): axum on a user-owned Unix socket. Status and a WebSocket of events, wake and sleep, typed turns, the pending change with Confirm and Cancel, audit, the apps (connect, disconnect, sign in) and the catalog, the permissions and the policy file, the passwords (names and metadata out, values only in, ADR 0036), the model connection (`/model`: its view, the provider's models, Test connection, OpenRouter's sign-in; ADR 0037), the schedules and their runs (`/schedules`, `/runs`; ADR 0047), devices and setup. `packages/api/openapi.json` is generated from its types (`make api-schema`) and `packages/client-ts` from that. Nothing passive crosses it |
| `ghost-core` | The write gate and what feeds it (ADR 0007, 0011, 0034, 0035): tool sources (`ghost_core::source::ToolSource`), the apps (`ghost_core::apps`), their catalog and sign-in (`ghost_core::catalog`, `ghost_core::signin`), the model connection (`ghost_core::connection`: the providers, which password holds the key, the settings from before it, ADR 0037), Ghost's folder and its rules (`ghost_core::folder`, `ghost_core::rules`), the operator's permissions (`ghost_core::permissions`, `ghost_core::policy`), proposals over typed targets and change sets, the confirmation classifier and Jev, the audit store, the vault (`ghost_core::passwords`) and redaction, the waiting list, watches, webhooks, routes, skills and outputs, the agents (ADR 0014), the schedules and their scheduler (`ghost_core::schedules`, ADR 0047), and Ghost's own app (`ghost_core::ghost_app`, ADR 0045). Depends on no other crate of the workspace. Its two binaries, `ghost-mock-github` and `ghost-scripts`, are the two apps `infra/example/apps.json` names |
| `ghost-core::conversations` | The agents (ADR 0014, ADR 0035): the registry mirrored from the folders, the transcript, every compaction version, and the files per agent under `~/.config/ghost/agents/<slug>/` (`agent.json`, `persona.md`, `memory.md`, `notes.md`). Their mutations are gate operations under `[conversations]` |
| `heka-ghost-desktop` | The shell (`apps/desktop/src-tauri`): the window, the tray, autostart, and the daemon's supervision (above); the window's bridge to the socket is typed by the same structs the daemon serializes |
| SQLite | Proposals, operations, operation events, the conversation registry, session transcripts, compaction versions, the items, the questions, the webhook deliveries, the schedules and their runs |

## Conversation flow

1. **Passive.** `ghostd` runs a short local pipeline: VAD, Parakeet, phrase matcher. A transcript without a wake phrase is dropped where it is produced. Nothing is stored or sent.
2. **Wake.** "Hey Ghost", "Ghost, you there?" or another configured phrase (`GHOST_WAKE_PHRASES`) matches (`ghost_infer::wake`). With `GHOST_WAKE_MODE=onnx` the trained detectors listen instead (ADR 0025, `ghost_voice::wakeword`): "hey ghost", and, when its model is installed, Ghost addressed by name with a request ("Ghost, what is the status of …"). A model fires as its phrase ends, Parakeet checks the hit (`GHOST_WAKE_VERIFY`), and the chime plays then, while the rest of the sentence is still being heard. `ghostd` plays a chime, ducks the configured music streams, and starts the session. Words spoken after the phrase in the same breath become the first turn, so "hey ghost, what needs me?" skips the greeting. `make wake` still triggers it by hand.
3. **Dialogue.** Silero VAD and Smart Turn decide when you have finished speaking; Ghost waits a short grace in case you go on, and the model starts answering during it, its answer held until the turn is confirmed and dropped if you keep talking (`GHOST_TURN_SPECULATE`; a held answer that reaches a tool call is withdrawn and the confirmed turn makes the call). The answer starts when the segment's words arrive, a moment after Smart Turn's verdict, never on the turn without them. Words that stop mid-thought ("can you count, um"), and a turn Smart Turn calls incomplete whose words don't end a sentence ("can you"), wait a longer silence (`GHOST_TURN_UNFINISHED_SECS`, 3 s); an incomplete turn whose words end with a full stop closes after `GHOST_TURN_STOP_SECS` (1.6 s), unless Smart Turn is sure it is not done (`GHOST_TURN_SENTENCE_MIN_PROB`, 0.1: Parakeet ends nearly every segment with a full stop). Going on after the turn was handed over, before any of the answer was heard or done, reopens it: the model gets the whole sentence as one turn, and nothing is done for half of it. Parakeet transcribes, the LLM answers or calls skill tools, and Kokoro speaks a clause at a time, so the voice starts after the first few words (`GHOST_TTS_CLAUSES`). Barge-in stops playback once the STT has heard at least two words from you (`GHOST_BARGE_IN_MIN_WORDS`); VAD alone never interrupts, so a leak of Ghost's own voice past the echo canceller cannot cut it off. While Ghost speaks the VAD starts quicker, on a quieter and less certain signal, and lets a segment run across a short gap (`GHOST_VAD_OVER_GHOST_*`), because the canceller passes you in 150–350 ms pieces under Ghost's audio; words too few to interrupt on their own count for two seconds, so "stop, stop, stop" cuts it; and anything heard in the first 0.8 s after Ghost's voice (re)starts is taken for Ghost's own, whatever the words, since that is when the canceller leaks. A hold phrase said over Ghost ("hang on") only stops it and keeps listening; the same words while Ghost is quiet put the session on hold (ADR 0021). While Ghost speaks, Parakeet re-reads your unfinished speech every 0.3 s (`GHOST_BARGE_IN_INTERIM_SECS`), so you needn't pause first; those interim words only ever decide barge-in, and the turn, the transcript and the model get the final transcript. When the model's reply begins with a call to a connected app (a read or a change, not Ghost's own quick tools), Ghost says a word or two first, "Sure." or "On it.", from a fixed list rendered ahead in its voice (`ghost_voice::acks`, `GHOST_ACK`): once a turn, never over the reply, and not a turn itself, so it reaches neither the transcript nor the model. An answer Ghost already has just comes. Every tool call in a reply runs, not only the last (a lesson from the first client, which kept one); and the model's output cap (`GHOST_LLM_MAX_TOKENS`, 2000 tokens) counts tool calls too, so a reply that hits it is redone in smaller steps rather than losing them (`ghost_llm`).
   A typed turn from the desktop app (`POST /session/say`) enters the same pipeline after
   the STT stage, so it meets the same tools and the same gate as speech.
4. **Transfer.** Every session opens in `ghost`. "Transfer me to Ledger" (an agent's name) is an LLM tool: Ghost's voice announces it, a short tone plays, the TTS voice and the system prompt change (the rules in `AGENTS.md`, the agent's persona and memory), and the context is replaced with that agent's rolling summary, pinned notes and recent turns; the agent greets with a one-line recap. "Back to Ghost" returns. "Save this as accounting" hires the agent and carries this session's turns into it. A transfer cancels any change waiting for a yes.
5. **Hold and hang up.** "Hang on, Ghost" puts the session on hold: the gate after STT drops everything the mic hears, unlogged, until an utterance that starts by addressing Ghost resumes it with the rest as the next turn; a hold ends quietly after 30 minutes (ADR 0021). A goodbye ("talk to you later", "bye", "that's all") or the idle timeout (45 s default) ends the pipeline, restores each music stream to its exact previous volume, and returns to passive. Every spoken turn was persisted as it happened; the next wake is `ghost` again.

### Retired agents

A retired agent leaves voice routing and the model's list, keeps its transcript, memory and
audit, and can be rehired.

### The waiting list (ADR 0029)

What needs the operator is one list (`ghost_core::waiting`): items with stable keys in the
`items` table, from watches, from Ghost itself (a change waiting for a yes, an app that
isn't connected or wants a new sign-in, a file in Ghost's folder with a problem) and from
the agents (`ghost_notify`, ADR 0045: one item at a time under `agent:<slug>`, cleared by
that agent's `ghost_resolve`, as loud as the urgency it was given; and what a scheduled
run answered, ADR 0047, one item per schedule), and from the outside
tools the operator connected (ADR 0048: under `client:<name>`, the same way). A watch or Ghost syncs the whole set it
sees; a new key is news, a key that is gone is resolved. An item is delivered when a route
passed it on and acknowledged when the operator heard it; only acknowledged or snoozed stops
the alerting, and a changed item alerts again. `GET /waiting`, `POST /waiting/ack` and
`POST /waiting/snooze` serve it, `Status.waiting` counts what is still to be heard, the bus
sends a `waiting` event, and the model reads it with `waiting_list` ("what needs me?"),
typed fields only.

### Questions (ADR 0051)

A question is an item with an answer (`ghost_core::questions`): on the waiting list under
its asker (`agent:<slug>`, `client:<name>`, `app:<name>`), with the rest in the
`questions` table: who asked, the words, up to four choices and whether only they will
do, the urgency, when it expires, and what came of it (answered with the operator's words,
the choice they clearly are, the route and when; declined; expired; taken back).
Answering is its acknowledgment and its resolution. An agent asks with `ghost_ask`, an
outside tool with `ask`, and a connected app with a one-field form (`AppAsker`). The
answer comes only from the operator: a click or their typed words in the desktop (route
`desktop`, `/questions/answer`, or the desktop notification's buttons), a signed request
from the paired phone (`phone`), or their voice in a session (`voice`, `ghost_answer`,
whose words the dispatcher checks against the operator's last turn). A choice is matched
by fixed rules, never by a model. A question asked from a run is followed up: its answer
queues one more run of the same agent with the question and the answer (once per
question, through the scheduler's once-per-key rule). A change waiting for a yes is shown
as the question Yes or No; Yes is the gate's own confirm by id (`ui` from the desktop,
`phone` from the phone). The bus sends a `question` event; `GET /questions` lists them.

### Routes (ADR 0031)

`ghost_core::routes::RouteEngine` ticks with the waiting list's refresh (every 15 s): for each
item still to be heard, outside quiet hours and do-not-disturb, past the batch window, it
picks the quietest enabled route the item's urgency allows (desk routes only when the
operator is at the desk: a desk session in the last 15 minutes, or the desktop app
connected), and after `escalate_after` minutes without an acknowledgment the next one up;
a route that knows nobody heard (desk voice with no reply), or that can't take the item
now (the desk busy, no `notify-send`, the phone app closed), hands it on at once. When a
snooze runs out the item starts again at the bottom of its ladder. A bare wake in Ghost
opens with the digest instead of a greeting ("Hi. Five things are waiting: …; and two
more"): the greeting going out delivers them, and the operator's first answer in that
session acknowledges only the three titles it said (`ghost_core::routes::SpokenDigest`).
"Nothing until two" is a fixed intent (no model) that sets do-not-disturb, kept in the
store over a restart; a time that isn't one ("until 3:75") is said back, not set. The
policy is `[routes]` in `policy.toml`; the model sets none of it.

Desk voice (`desk`, opt-in, ADR 0032) is the daemon's own route (`ghostd::routes::DeskRoute`):
only when the desk is idle, the passive loop takes it like a wake, releases the wake
listener, and opens a session that greets with `[routes].greeting` and nothing about the
items, which wait in the context (the first five titles). The phase is `asking`. A reply
within 15 seconds acknowledges those five; "not now" snoozes them for 30 minutes; silence
ends the session with nothing kept, and the engine hands the items to the next route at
once. A desktop notification's button acknowledges what it showed, unless the item changed
after; a question alone in its notification has its choices as the buttons, and a click
is the operator's answer.

A question in the digest or in desk voice's note is put, not announced (ADR 0051): "Ledger
asks: should the October invoice go out today? Yes or no?", last in the line, and the
model's note carries its key and says to record the operator's exact words.

The phone route (`ghostd::routes::PhoneRoute`) sends an `alert` on the phone's live stream
and is delivered only when the phone answers `POST /seen` within 15 seconds; with no
stream open it is unavailable at once. The call route (`CallRoute`, `call` in
`[routes].enabled`, the call caps apply) rings the phone for the first item at `call`
urgency: answered, the phone opens a session through `/offer` with the agent and the
item, which Ghost opens with, and that is the acknowledgment; declined, the item is
snoozed; unanswered after 30 seconds, nobody heard. The ntfy route (`NtfyRoute`, `ntfy` in
`[routes].enabled`, the topic's address and token named as passwords in `[routes]`) posts
a count and the kinds of source to the operator's ntfy topic, titles only with
`carry = "title"`, at most `ntfy_per_hour` (4); it can never be acknowledged and is last on
any ladder louder than the digest. It is plain HTTPS from the routes code: the one
deliberate exception to "never a bespoke client" (ADR 0051).

### Schedules and runs (ADR 0047)

An agent can work with nobody there. A **schedule** is one job in SQLite (`schedules`,
`ghost_core::schedules`): a name, the agent, an instruction, and when, which is `at` a
time, `every` interval (`30m`, `2h`, `1d`) or a `cron` expression of five fields in a named
time zone (`croner` over `chrono-tz`; the machine's zone by default). It may have active
hours, a grace window, limits, skills to start with, an urgency, and a check. The operator
adds one on the desktop's Watching screen (`/schedules`); an agent told "check the build
every morning at nine" adds its own with `ghost_schedule_manage`, through the gate.

A **run** is one execution (`ghost_voice::headless::HeadlessRunner`): the agent's prompt
written for reading (`ghost_voice::prompt::SYSTEM_ALONE`: the same identity, rules, persona
and apps' paragraphs as a session's), its memory, notes and recent turns, the model
through `ghost-llm`, and the session's own `ToolDispatcher` built with
`ToolDispatcher::alone`, so every read and every change takes the path a session's does.
No audio stage exists in it, and the session's own tools (hang up, hold, transfer, bring
up a change) are neither offered nor callable. It loops until a reply has no tool call,
for 30 model calls (the last is offered no tools) and ten minutes at the most, stops when
cancelled, and tries a model it cannot reach again after 30, 60 and 120 seconds. A change
that resolves to `ask` is set aside on the waiting list at once (ADR 0030): nobody is
there, and the instruction is never taken for the operator's yes.

The scheduler (`ghost_core::schedules::Scheduler`, started by `ghostd::schedules`) looks
every 15 seconds and when something is queued: it claims what is due, writes the run's row
(`schedule_runs`) before the run starts, and runs one job at a time per agent, taking
turns with the asks of that agent (`agent_ask`, `ask_agent`), which go first and give up
as busy after 60 seconds (`ghost_core::schedules::turns`). With a
check, it first calls the app's read-only tool through the watches' read path
(`AppReader`, the permissions' `block` rules apply), hashes the result with its keys in
order, and runs the agent only when the hash changed, telling it which lines did. After
Ghost was off, a schedule missed by less than its grace window (two hours) runs once,
however many times it missed; later than that it is recorded as missed. A run that was
going when Ghost stopped is marked interrupted and is not run again.

The answer is an item on the waiting list under `agent:<slug>`, one per schedule, the
answer as its detail and as loud as the schedule says, so the routes deliver it. Exactly
`[SILENT]`, or nothing, delivers nothing. A failed run delivers a failure item unless the
schedule says not to. When the agent called `ghost_notify` itself, the answer is not a
second item. The run's turns go into the agent's transcript under its own session id
(`run-<id>`), marked `[scheduled run: <name>]`; one with nothing to tell is a single line
the model's context does not carry.

A watch may act (ADR 0029): `"response": "act"` with an `act` block starts a run of the
named agent for each new item, given the item's typed fields and nothing else, once per
item and twelve times an hour at the most. A run happens beside an open session and
shares nothing mutable with it: no microphone, no speaker, no listener on the event bus.

### Memory and compaction

Each conversation loads only its own persona, memory and notes. The memory is a rolling summary kept in `memory.md`, produced by the session's own summarizer (`ghost_voice::llm::Compaction`) with a tokens-only trigger (24k estimated tokens, last 20 messages kept raw, summary up to 3k; `GHOST_COMPACT_*`). Each applied summary is recorded as a version and written to the file through the gate; the file is the source of truth on resume, so an edit by hand is folded into the next version. Summarized turns stay in SQLite, flagged, and are kept for recall unless `GHOST_TURN_RETENTION_DAYS` sets a window (0, keep, by default); hidden turns (a reset or a delete) are pruned after a week, and pruning runs at start and then daily (ADR 0041). Raw tool results are never stored, so old untrusted text does not re-enter a later session. Every stored turn is in a full-text index (`turns_fts`, FTS5, kept in step by triggers, built once when an older database opens; ADR 0041), which `ghost_recall` searches: a turn added is found at once, a compacted one until it is pruned, a hidden one never.

## Writes

Speech-to-text runs inside our own process, so the backend observes the transcribed
user turn directly. `core` accepts a confirmation only from a transcript frame the
pipeline produced for the current session, never from LLM output. What Ghost does without
asking is the operator's call (ADR 0011, ADR 0034); by default, everything:

1. The LLM calls a change: an app's tool under its own name (`github_issue_write`), or
   `change` for one of Ghost's own operations. The source's `prepare` turns the arguments
   into a target and a typed change set: for an app, checked against the tool's own schema
   and bounded (200,000 characters a string, 1,000 items a list), with the words that would
   ask for it read off the tool's name and short arguments.
2. `core` checks the source's own limit, then resolves the level from the permissions for
   this source, tool and arguments: the most careful match among the operator's rules,
   else the mode (`open` announces; for Ghost's own app it is quiet, ADR 0045).
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
   number, the sent message) comes back as the read-back, within the app's result budget.
   A call stopped because the operator spoke or hung up is recorded as failed, and the
   model is told the change may or may not have been made.
6. The model says what happened in a few words, or asks one short question when the
   change waits. There is no undo (ADR 0015): asked to change something back, Ghost asks
   for the opposite change like any other.

## Repository layout

```text
ghost/
├── crates/
│   ├── ghostd/          # the daemon: wake, sessions, apps, the control API server, the phone, hooks and outside-tools listeners
│   ├── ghost-voice/     # Ghost's pipeline: the stages, the harness behind make perf
│   ├── ghost-llm/       # the model client, both protocols
│   ├── ghost-infer/     # the local models on ONNX Runtime
│   ├── ghost-audio/     # PipeWire, ducking, the sounds
│   ├── ghost-rtc/       # the phone's WebRTC media
│   ├── ghost-api/       # the control API on the Unix socket; its types make openapi.json
│   └── ghost-core/      # tool sources, apps, catalog, sign-in, Ghost's folder, gate, permissions, confirmation, audit, vault, conversations
├── apps/
│   ├── desktop/         # Tauri 2 + React: the window, the tray, the daemon's supervision (src-tauri, a workspace member); chat, confirmation card, settings
│   └── mobile/          # Expo + React Native, Android: Home, session, activity
├── packages/
│   ├── api/openapi.json # the contract, generated from crates/ghost-api (make api-schema)
│   ├── client-ts/       # TypeScript: GENERATED from packages/api/openapi.json (pnpm gen)
│   └── ui/              # TypeScript: React pieces shared by desktop and mobile
├── tools/
│   ├── version.py, tests/   # the version script and the scripts' tests (the root uv workspace)
│   └── oracle/          # the Python the Rust is measured against: the model wrappers, pinned (its own uv lock)
├── scripts/             # dev.sh, deploy.sh, sidecars.sh, onnxruntime.sh; fixtures/ (the parity recorder), rtc/ (the aiortc far end)
├── infra/
│   ├── example/         # Ghost's seed folder: apps.json, AGENTS.md (make env)
│   ├── systemd/         # ghostd.service, the headless mode (a make deploy build)
│   └── pipewire/        # echo-cancel config
└── docs/
```

Three toolchains, one root: a Cargo workspace for the daemon and the shell, a `pnpm`
workspace for TypeScript, and `uv` for the Python tools. They never import each other; the
only crossing is `packages/api/openapi.json` → `packages/client-ts`, generated from the Rust
types and checked by CI.

Crate direction is strictly `ghostd -> {ghost-voice, ghost-api, ghost-rtc, ghost-audio,
ghost-infer, ghost-llm} -> ghost-core`; `ghost-core` depends on no other crate of the
workspace, and nothing knows a connected app by name: `ghostd` connects whatever `apps.json`
lists, and serves Ghost's own (`ghost`) beside them.
See [apps.md](apps.md) to connect one.

### How the gate is wired into the pipeline

```text
capture -> [speech-in: Silero VAD, Parakeet] -> interceptors -> confirmation -> [the user's turn: Smart Turn, the grace]
  -> [model: ghost-llm, the tools] -> narration cut -> spoken filter -> [TTS: Kokoro a clause at a time] -> playback
```

Each stage is one task over a bounded channel, one fresh set per session
(`ghost_voice::pipeline::build_session`). A frame waits in a stage's inbox behind that
stage's current work, so whether Ghost is speaking is also shared by handle
(`ghost_voice::speaking`): the output stage raises it at a reply's first sample, and the
barge-in rule, the interim re-reads, the echo filter and the confirmation processor read it
there rather than from the `BotStarted` frame queued behind a TTS stage still rendering the
reply. The echo filter is also told each clause as it is rendered, a clause ahead of the
speaker, so what leaks past the canceller is recognised as Ghost's own words at once.
The interceptors (`ghost_voice::intercept`) see
each user turn first (Ghost's own fixed intents: a goodbye, "hang on", "never mind" while a
change is pending); a handled turn is spoken to or acted on directly and never reaches the
model.

The confirmation processor (`ghost_voice::confirmation`) sees every final transcript. If
the gate holds a pending proposal for the session, the frame's text is the confirmation
evidence, judged with Ghost's last reply as context: a clear yes executes, anything else
cancels. Transcripts that arrive while Ghost is still speaking are not evidence at all: the
operator has not heard the question yet, and the text may be Ghost's own voice. The
verified outcome is appended to the context as a system note, so the model's next sentence
describes what actually happened. The model's tools are every connected app's tools (reads
called directly, changes through the gate), Ghost's own app's in the same way (`ghost_*`,
ADR 0045), the agents', the skills' and the waiting list's read tools, `change`, `hang_up`
and `hold`. There is no change that bypasses the gate.

The desk's audio is `crates/ghost-audio`: PipeWire through the project's own binding, one
clock domain, a capture thread that never allocates. Ghost listens on `ghost.mic`, the output
of PipeWire's WebRTC echo canceller running as its own user service
([infra/pipewire](../infra/pipewire/README.md)). The module takes the speaker sink as its
reference, so what Kokoro plays is subtracted from what the microphone hears before VAD or
STT see it. Device names are rendered into the user's config at install time and never
committed.

## Open items

- Wake word: the transcript matcher costs continuous local STT while passive. Measure its CPU and false wakes against music; if either is a problem, train "Hey Ghost" with `livekit-wakeword` for the `OnnxWakeDetector` (ADR 0005, ADR 0010).
- Real-voice benchmark on the workstation mic, with music playing ([benchmarks](benchmarks/2026-10-16-speech-cpu.md) used synthetic clips).
- LLM default: `claude-sonnet-5-5` with `thinking: {"type": "between_tools"}` (0.77 s to first token; see the [benchmark](benchmarks/2026-10-16-speech-cpu.md#llm-time-to-first-token)). Where a turn's time goes, measured: [2026-10-19](benchmarks/2026-10-19-latency.md), and the Rust pipeline against it: [2026-10-29](benchmarks/perf/2026-10-29-rust-voice.md).
- A dedicated LLM API key for Ghost, so it can be revoked on its own: signing in to OpenRouter under Settings › Model gives Ghost a key of its own (ADR 0037). The connection is one provider for everything; a cheaper model for compaction or a watch's step would be a second one, not yet asked for.

## Shipping

One bundle (ADR 0038): `tauri build` makes a .deb and an AppImage carrying the desktop app,
`ghostd`, `ghost-mock-github`, `ghost-scripts`, the ONNX Runtime library, the four small
model files (Silero, Smart Turn, the wake front end) and the build's record. `make bundle`;
CI builds it on every push and the release workflow attaches it to a release
([releasing.md](releasing.md)). Parakeet, Kokoro and the trained wake classifiers are not in
the package; [releasing.md](releasing.md#the-speech-models) says how each gets to its place.
Passwords are never in the package or in a plain file: they are in Ghost's vault, its
key in the keyring (below). Target is Ubuntu 26.04 amd64.

## Passwords (ADR 0036)

Every API key, token and account password is a *password*, kept in one file,
`~/.local/share/ghost/passwords.vault` (0600, AES-256-GCM; `ghost_core::passwords`). Its key
is one item in the operator's login keyring through Secret Service. `ghostd` opens the vault
at start (`ghostd::passwords::open_passwords`; so do `check`, `perf` and `push`); a locked
keyring is retried every 10 s with "Unlock your keyring" on the waiting list, and when it
opens the voice settings, Jev and the apps that waited follow. The values are served from
memory: through a mapping that notes each use where an app connects or a webhook is checked,
and in the daemon's own environment for Ghost's settings. A stdio app gets the ones its `env`
names; `child_env()` keeps them from everything else Ghost starts. Every value of 8
characters or more is replaced with `[password: NAME]` in app results, errors and tool
descriptions, in what the store writes, and in every log line. The control API's
`/passwords` lists names, what uses each and when it was last used, and takes values that
never come back. `env` and `apps.env` from before are read until the one-time import
(`ghostd passwords import`, the Passwords page) moves them and heka-vault's values into the
vault and deletes them; the service and the dev scripts load `settings.env` only.
