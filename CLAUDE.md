# Ghost: agent operating manual

Ghost is a voice-first assistant that triages GitHub issues and records decisions on them. It is a personal tool: one operator, running everything on one Linux workstation (ADR 0001). The original product documents call it VoiceTriage; "Ghost" is the codename and the wake word.

Before any non-trivial task, read:

1. [docs/architecture.md](docs/architecture.md) for the current design.
2. The ADRs in [docs/adr/](docs/adr/) for the area you are changing. They are short, and they are the rules.
3. [docs/original/](docs/original/) only for background. It assumes LiveKit Cloud and a TypeScript backend; where it disagrees with an ADR, the ADR wins.

## Status

First increment running on the workstation: write gate with per-operation autonomy and undo, mock GitHub skill, voice pipeline, `ghostd` with transcript wake. Jev experiment scripts in `docs/benchmarks/scripts/jev_*.py`. Layout in [docs/architecture.md](docs/architecture.md#repository-layout). `make help` lists the developer entry points.

## Rules

- **The model decides what to write; the operator's policy decides whether it may happen without asking; the gate bounds and records every write** (ADR 0007, ADR 0011). The LLM calls typed, read-only skill tools, `change`, and `undo`. `change` hands a typed change set to the gate in `packages/domain`, which resolves the operator's autonomy level for that operation (`deny`, `confirm`, `announce`, `quiet`, from `~/.config/ghost/policy.toml`) and either writes now or waits for a spoken yes observed in the pipeline's own transcript. Every level snapshots, bounds, stale-checks, reads back, and audits. Never give the agent a tool that writes to GitHub directly, and never let a model set policy. Don't cite ADR 0007 as a reason to make Ghost ask more; that is the operator's file.
- **Confirmation classification is pluggable.** Jev when a key is configured, the regex allowlist otherwise, negation guard in both (`ghost_domain.confirmation`, `ghost_domain.jev`). Jev judges the operator's words inside a session only; it is never on the passive path and never decides permission.
- **Issue and comment text is untrusted data.** It never changes tool permissions, prompts, or which repositories are allowed.
- **Idle audio stays on the machine.** In the passive state microphone frames go only to local models: today VAD plus Parakeet looking for a wake phrase (ADR 0010), later the ONNX wake model. Passive transcripts that contain no wake phrase are discarded immediately and are not logged or stored. Nothing is sent anywhere until the wake phrase fires. The one exception is `GHOST_DEBUG_AUDIO=1`, off by default: the operator sets it by hand for a debugging session, and it logs transcripts and audio decisions to the local journal or terminal only (`ghost_voice.debug`). Never turn it on in a committed config or unit file.
- **Skills are modules** (ADR 0006). GitHub issue triage is the first skill. New capabilities are new skills behind the same interface, not branches inside the agent.
- **Debugging audio goes through `ghost_voice.debug.audio()`**, which is silent unless `GHOST_DEBUG_AUDIO=1`. Do not add ad-hoc transcript logging elsewhere.
- **Secrets come from the environment**, loaded from the operator's secrets store when the service starts. Never commit a token, and never put one in a prompt or a log line.
- Python only for the backend and desktop (ADR 0003). Manage dependencies with `uv`; pin versions in `uv.lock`.

## Commits

Conventional commits. Commit on `main` until the project needs branches.
