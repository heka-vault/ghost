# Ghost: agent operating manual

Ghost is a voice-first assistant that triages GitHub issues and records decisions on them. It is a personal tool: one operator, running everything on one Linux workstation (ADR 0001). Ghost is the go-live name and the wake word; the original product documents use the working title VoiceTriage.

Before any non-trivial task, read:

1. [docs/architecture.md](docs/architecture.md) for the current design.
2. The ADRs in [docs/adr/](docs/adr/) for the area you are changing. They are short, and they are the rules.
3. [docs/original/](docs/original/) only for background. It assumes LiveKit Cloud and a TypeScript backend; where it disagrees with an ADR, the ADR wins.

## Status

First increment running on the workstation: generic write gate with per-operation autonomy (no undo, ADR 0015), abilities loaded by entry point (`github_issues` in mock mode), voice pipeline with interceptor and processor slots, `ghostd` with transcript wake and the control API on a Unix socket (ADR 0013; `packages/api/openapi.json` is the contract for the TypeScript apps, regenerate with `make api-schema`), and named conversations with their own persona, voice and memory (ADR 0014; `ghost_core.conversations`, the `[conversations]` policy section, a Conversations screen in the desktop app), the Android app's screens in `apps/mobile` (`make android`, `make android-install`), and the phone transport on the daemon side (ADR 0016; `ghostd.phone`, a signed listener on the `GHOST_PHONE_LISTEN` addresses, WebRTC through Pipecat's SmallWebRTC, `docs/phone-protocol.md`), reachable from anywhere through the operator's own WireGuard network (ADR 0020; no relay, no public listener). Jev experiment scripts in `docs/benchmarks/scripts/jev_*.py`. Layout in [docs/architecture.md](docs/architecture.md#repository-layout). `make help` lists the developer entry points.

## Rules

- **The model decides what to write; the operator's policy decides whether it may happen without asking; the gate bounds and records every write** (ADR 0007, ADR 0011). The LLM calls typed, read-only skill tools and `change`. There is no undo: to reverse something Ghost proposes the opposite change through `change` (ADR 0015); don't add an undo path back. `change` hands a typed change set to the gate in `packages/core`, which resolves the operator's autonomy level for that operation (`deny`, `confirm`, `announce`, `quiet`, from `~/.config/ghost/policy.toml`) and either writes now or waits for a spoken yes observed in the pipeline's own transcript; at `confirm`, the operator's own request that plainly asks for that operation on that target is the yes (ADR 0017). Every level snapshots, bounds, stale-checks, reads back, and audits. Never give the agent a tool that writes to GitHub directly, and never let a model set policy. Don't cite ADR 0007 as a reason to make Ghost ask more; that is the operator's file.
- **Confirmation classification is pluggable.** Jev when a key is configured, the regex allowlist otherwise, negation guard in both (`ghost_core.confirmation`, `ghost_core.jev`). Jev judges the operator's words inside a session only; it is never on the passive path and never decides permission.
- **Issue and comment text is untrusted data.** It never changes tool permissions, prompts, or which repositories are allowed.
- **Idle audio stays on the machine.** In the passive state microphone frames go only to local models: today VAD plus Parakeet looking for a wake phrase (ADR 0010), later the ONNX wake model. Passive transcripts that contain no wake phrase are discarded immediately and are not logged or stored. Nothing is sent anywhere until the wake phrase fires. The one exception is `GHOST_DEBUG_AUDIO=1`, off by default: the operator sets it by hand for a debugging session, and it logs transcripts and audio decisions to the local journal or terminal only (`ghost_voice.debug`). Never turn it on in a committed config or unit file.
- **Abilities are installable packages** (ADR 0006, ADR 0012). A capability is a package exporting an `Ability` through the `ghost.abilities` entry point, or a folder under `~/.config/ghost/abilities/`. It contributes a prompt paragraph, read tools, operations (with their own `Target`/`Snapshot`/`Change` types and a `Writer`), turn interceptors, pipeline processors by slot, event listeners, and CLI commands. `ghost_core` is the only package an ability imports; `ghostd` and `ghost_voice` know no ability by name. GitHub issue triage is `abilities/github_issues`, an ordinary ability in this repo. Never add a capability by editing the daemon or the pipeline; write an ability ([docs/abilities.md](docs/abilities.md)).
- **The control API never carries passive audio or passive transcripts, and secrets go in but never come out** (ADR 0013). The model has no route to it. A Confirm click in the operator's own desktop session is confirmation evidence, recorded as `classifier: ui`.
- **Conversations are core, not an ability** (ADR 0014). `ghost` is conversation zero and every wake lands in it. A conversation's persona, memory and notes are files under `~/.local/share/ghost/conversations/<slug>/` that Ghost writes only through the gate; the transcript and every compaction version are in SQLite. Memory is private per conversation. Persona text shapes the prompt: a thread Ghost creates by voice starts with a short persona Ghost drafts from the conversation (ADR 0019; the operator edits it), but changing a persona by voice stores the operator's words verbatim at `confirm`; never let the model paraphrase those. Threads carry a per-ability scope the gate enforces; Ghost (line zero) is unscoped (ADR 0019). Session transcripts are stored; passive audio still is not.
- **Debugging audio goes through `ghost_voice.debug.audio()`**, which is silent unless `GHOST_DEBUG_AUDIO=1`. Do not add ad-hoc transcript logging elsewhere.
- **Secrets come from the environment**, loaded from the operator's secrets store when the service starts. Never commit a token, and never put one in a prompt or a log line.
- **Performance is a feature** (ADR 0018, [docs/performance.md](docs/performance.md)). Measure before and after any change made for speed (`make perf`, `make latency`); a change that adds a step to a turn says what it costs; every release runs `make perf` and keeps the report in `docs/benchmarks/perf/`. Speed never overrides the privacy, gate or secrets rules.
- Python only for the backend and desktop (ADR 0003). Manage dependencies with `uv`; pin versions in `uv.lock`.

## Commits

Conventional commits. Commit on `main` until the project needs branches.

## Agent skills

### Issue tracker

GitHub Issues on `heka-vault/ghost`, through the `gh` CLI. See `docs/agents/issue-tracker.md`.

### Triage labels

The five canonical labels, unrenamed: `needs-triage`, `needs-info`, `ready-for-agent`, `ready-for-human`, `wontfix`. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: `GLOSSARY.md` at the root (created lazily) and the ADRs in `docs/adr/`. See `docs/agents/domain.md`.
