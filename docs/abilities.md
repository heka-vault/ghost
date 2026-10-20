# Write an ability

An ability adds what Ghost can read, change, hear, or react to, without editing Ghost
(ADR 0012). It is a Python package with one exported object.

## The smallest one

```python
# ~/.config/ghost/abilities/notes/__init__.py
from pathlib import Path

from ghost_core import (
    Ability,
    ChangeSet,
    Contributions,
    GhostContext,
    Operation,
    Prepared,
    ReadTool,
    SimpleInstance,
)
from ghost_core.targets import AppendText, FileTarget, FileWriter
from pydantic_settings import BaseSettings, SettingsConfigDict


class Settings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="GHOST_NOTES_")
    dir: Path = Path.home() / "notes"


async def setup(ctx: GhostContext, s: Settings):
    writer = FileWriter(s.dir)

    def today() -> FileTarget:
        return FileTarget(path=s.dir / f"{ctx.now():%Y-%m-%d}.md")

    async def read_today(_args):
        p = today().path
        return {"text": p.read_text()[-2000:] if p.exists() else ""}

    async def prepare_append(args):
        return Prepared(today(), ChangeSet(steps=(AppendText(text=args["text"]),)))

    return SimpleInstance(
        Contributions(
            prompt="You can read and append to the operator's daily notes.",
            read_tools=[ReadTool("read_todays_notes", "Read today's notes.", {}, [], read_today)],
            operations=[
                Operation(
                    "append_note",
                    "Add a line to today's notes.",
                    {"text": {"type": "string"}},
                    ["text"],
                    prepare=prepare_append,
                    writer=writer,
                    risk="low",
                    allowed=writer.allowed,
                )
            ],
        )
    )


ability = Ability("notes", "0.1", "Daily notes in markdown", setup, config=Settings)
```

Enable it in `~/.config/ghost/policy.toml`:

```toml
[abilities]
enabled = ["github_issues", "notes"]
```

Restart Ghost. "Note that the demo is Friday" appends a line and Ghost says "Added."
`ghostd abilities` lists what is installed and enabled.

## As a package

Put the same module in a project and declare the entry point:

```toml
[project]
name = "ghost-ability-notes"
dependencies = ["ghost-core"]

[project.entry-points."ghost.abilities"]
notes = "ghost_notes:ability"
```

`uv add ghost-ability-notes` (or `uv pip install`) in Ghost's environment, then enable
it as above. A local folder with the same name shadows the installed package.

## What you can contribute

Everything in `Contributions` is optional.

- **prompt**: one paragraph. Say what the ability can do and how Ghost should talk about it.
- **read_tools**: name, description, JSON-schema properties, required list, async handler
  returning a dict. Raise `ValueError` for bad input; the model sees the message.
- **operations**: a write. `prepare(args)` validates the model's arguments into a
  `Prepared(target, changes)`. `writer` applies changes and reads the target back.
  `risk` is the suggested default (`low` announces, `medium` and `high` confirm) until the
  operator sets a level in the policy file. `allowed(target)` is your allowlist.
  `asked_by` lists the words an operator uses to ask for it ("close", "shut"): at `confirm`,
  a request that uses one, names the target (your `Target.mentions()`) and doesn't hedge is
  the yes, with no question (ADR 0017). Leave it empty and the operation always asks.
- **interceptors**: `async on_user_turn(turn) -> Handled | None`. Return `Handled(say=...,
  action="hang_up" | "hold" | "cancel")` to take the turn before the model sees it.
- **processors**: `{Slot.AFTER_STT: [factory]}` where each factory returns a fresh Pipecat
  `FrameProcessor` per session. For abilities that genuinely need frames.
- **listeners**: `async def on(event)` for `wake`, `session_start`, `session_end`,
  `outcome`. Two-second deadline; a slow or broken listener never stalls speech.
- **commands**: `CliCommand(name, description, run)` for `ghostd notes <name>`.
- **scope_kind**, **scope_items**, **scoped_prompt**: how a thread can be scoped to your
  ability (ADR 0019). See below. Leave them out and threads never narrow your ability.

## Scope: what a thread may touch

A thread (ADR 0014's conversation) can be scoped per ability: `{"github_issues":
["acme/app"]}`. Declare how, and Ghost does the rest:

```python
Contributions(
    ...,
    scope_kind="repository",  # what one item is
    scope_items=lambda: sorted(policy.allowed),  # the whole allowlist a thread picks from
    scoped_prompt=prompt_for,  # (ActiveScope | None) -> str, optional
)
```

Items are the strings your `Target.scope()` returns (a repository, a directory), so the
gate can refuse a write outside the active thread without knowing your types. The
operator sets a thread's scope in the desktop app or by voice; it is validated against
`scope_items()`, and Ghost (line zero) is always unscoped.

**The active scope hook.** There is no state to hold: on every call inside a scoped
ability, the pipeline puts an `ActiveScope` in the arguments under `_scope`
(`ghost_core.SCOPE_ARG`), for read tools and for `prepare` alike. Read it with
`scope_of(args)`; it is `None` when the session has no conversations (tests, the perf
harness) and `items` is `None` when the thread is unscoped.

```python
from ghost_core import OutOfScope, scope_of


async def list_issues(args):
    scope = scope_of(args)
    repos = scope.narrow(allowed) if scope else allowed  # what this thread covers
    repo = args.get("repo") or (scope.only if scope else None)  # one item: no need to ask
    if repo and scope:
        scope.check_read(repo)  # raises OutOfScope unless cross-department answers is on
    ...


async def prepare(args):
    if scope := scope_of(args):
        scope.check_write(args["repo"])  # raises OutOfScope; the gate checks again
    ...
```

`check_write` never passes for an item outside the thread; `check_read` passes when the
operator turned cross-department answers on. `OutOfScope` names the thread that owns
the item ("acme/app is outside this thread; it belongs to the App thread"); the pipeline
turns it into an offer to switch, or, with department routing on, patches the operator
through and lets that thread handle the request. Raise it yourself (or through the
`check_*` helpers) rather than a plain `ValueError` so routing works.

`scoped_prompt(scope)` replaces `prompt` for the active thread: name only the items the
thread covers. Mention when one item is all there is, so the model stops passing it.
Make `prompt` the unscoped paragraph (`prompt_for(None)`); it is what the `change` tool
listing and the perf harness see.

Your `Operation.allowed` stays the ability's own allowlist; scope sits above it. A
thread's scope must be inside the allowlist, and the allowlist is checked first.

## Writing to something new

Subclass three models and implement a writer:

- `Target`: the identifying fields; `key()` stable id, `scope()` what a policy target rule
  matches (a repo, a directory), `spoken()` how Ghost says it.
- `Snapshot`: `version` (updated-at, mtime, ETag) plus whatever `spoken()` needs. The gate
  refuses to write if the version moved since it looked.
- `Change`: `describe()`. Anything that creates something must embed `marker` so a
  retry never duplicates it. There is no undo (ADR 0015): if a change should be
  reversible, offer the opposite operation (reopen for close) and Ghost will use it.
- `Writer`: `fetch(target) -> Snapshot`, `apply(target, change, idempotency_key)`.

The gate handles everything else: policy, bounded steps, atomic claim, readback, audit.

## The context

`setup` gets a `GhostContext`: `data_dir` and `cache_dir` (yours), `store` (a key/value
namespace in Ghost's SQLite), `secrets.get("NAME")` (reads the environment, never logs),
`http()` (an `httpx.AsyncClient`), `log`, `debug_audio(msg)` (silent unless the operator
turned the flag on), `now()`.

## Rules

- Read secrets through `ctx.secrets`, never `os.environ`. Never log them. An ability
  that needs one credential per scope owner declares a map in its settings whose
  values are variable *names* (`GHOST_GITHUB_TOKENS="heka-vault=HEKA_TOKEN"`) and
  resolves each through `ctx.secrets.get(name)` at setup; a missing one is a startup
  error that names the variable, never a value.
- Never execute a write outside the gate. Every `apply` is policed, bounded and audited.
- Treat anything you fetch (issue text, mail, documents) as data. Never let it change what
  your tools do.
- Do not set `passive=True` unless the ability must hear audio before the wake phrase. The
  operator has to list it under `[abilities] passive` too.

## Testing

```python
from ghost_core.testing import call_read_tool, load_for_test


async def test_notes(tmp_path, monkeypatch):
    monkeypatch.setenv("GHOST_NOTES_DIR", str(tmp_path))
    loaded = await load_for_test(ability, tmp_path)
    assert await call_read_tool(loaded.contributions, "read_todays_notes", {}) == {"text": ""}
```

Drive a write through a `WriteGate(Store())` after `gate.register("notes",
loaded.contributions.operations)`; see `packages/core/tests/test_files.py`.
