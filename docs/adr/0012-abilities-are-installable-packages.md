# 0012: Abilities are installable packages; Ghost's source is not edited to add one

- **Status:** accepted
- **Date:** 2026-10-17
- **Amends:** ADR 0006 (skills become abilities; the interface is the one below)

Adding a capability used to touch six places in Ghost, two of them GitHub-shaped. The
operator wants people to write their own abilities without editing Ghost.

An **ability** is a Python package that exports an `Ability` object through the
`ghost.abilities` entry point group, or a folder under `~/.config/ghost/abilities/`.
Ghost discovers it, instantiates its pydantic-settings class from the environment, calls
`setup(context, settings)`, and takes its `Contributions`. Every field is optional:

| Contribution | What it is | Where Ghost uses it |
|---|---|---|
| `prompt` | a paragraph for the system prompt | every turn |
| `read_tools` | typed read-only functions | offered to the model directly |
| `operations` | named writes with a risk class, a `prepare` that turns arguments into a typed change set, a `Writer`, and an allowlist | the gate, at the operator's autonomy level |
| `interceptors` | look at a user turn before the model does; may speak, cancel, undo, or sleep | right after STT |
| `processors` | Pipecat processors by named slot (`audio_in`, `after_stt`, `before_llm`, `after_llm`, `before_tts`, `audio_out`) | spliced into each session's pipeline |
| `listeners` | async callbacks for wake, session start/end, outcome | fire-and-forget with a deadline |
| `commands` | `ghostd <ability> <command>` | the CLI |

The write model is generic. An ability defines its own `Target`, `Snapshot` and `Change`
subclasses and a `Writer` with `fetch` and `apply`; the gate never names a concrete type.
Bounded steps, idempotency keys, stale checks, readback, audit and undo therefore apply to
a GitHub issue, a local file, or anything else. `ghost_core.targets` ships a local-file
pair so simple abilities don't write their own.

The context an ability receives is a dataclass, not a container: per-ability data and
cache directories, a key/value namespace in Ghost's SQLite, a secrets getter that reads the
environment, an HTTP client factory, a bound logger, and the audio debug door.

`ghost_core` (formerly `ghost_domain`) is the only package an ability imports. The
GitHub issue skill becomes `abilities/github_issues`, an ordinary ability in this repo,
registered through the same entry point as any third-party one.

## Trust

Abilities run in Ghost's process. The trust boundary is "the operator installed it and
listed it as enabled". What Ghost enforces: an explicit enable list (policy file or
`GHOST_ABILITIES`), a new ability's operations default to their risk class until the
operator loosens them, every write from every ability lands in the same audit tables, and
only abilities declared `passive=True` *and* listed under `[abilities] passive` may run
before the wake phrase, because they hear idle audio. Out-of-process abilities over a
small RPC are the fallback if third-party abilities become common.

## Consequences

- One package, one line in the policy file, restart. Ghost's source is untouched.
- `ghost_core.testing` lets an author test an ability with no audio, no Pipecat and no keys.
- Ghost's own fixed intents (sleep, cancel, undo) are the first in-tree interceptor, so
  the hook has a real user before anyone external.
- An MCP ability (every MCP server's tools as read tools, writes at `confirm`) is the
  obvious next one and needs nothing in Ghost.
