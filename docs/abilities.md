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
"Undo that" removes it. `ghostd abilities` lists what is installed and enabled.

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
- **interceptors**: `async on_user_turn(turn) -> Handled | None`. Return `Handled(say=...,
  action="sleep" | "cancel" | "undo")` to take the turn before the model sees it.
- **processors**: `{Slot.AFTER_STT: [factory]}` where each factory returns a fresh Pipecat
  `FrameProcessor` per session. For abilities that genuinely need frames.
- **listeners**: `async def on(event)` for `wake`, `session_start`, `session_end`,
  `outcome`. Two-second deadline; a slow or broken listener never stalls speech.
- **commands**: `CliCommand(name, description, run)` for `ghostd notes <name>`.

## Writing to something new

Subclass three models and implement a writer:

- `Target`: the identifying fields; `key()` stable id, `scope()` what a policy target rule
  matches (a repo, a directory), `spoken()` how Ghost says it.
- `Snapshot`: `version` (updated-at, mtime, ETag) plus whatever `spoken()` needs. The gate
  refuses to write if the version moved since it looked.
- `Change`: `describe()` and `invert(before, marker)`. Return the change that puts
  `before` back, or `None` if it can't be undone. Anything that creates something must
  embed `marker` so a retry never duplicates it and undo can find it.
- `Writer`: `fetch(target) -> Snapshot`, `apply(target, change, idempotency_key)`.

The gate handles everything else: policy, bounded steps, atomic claim, readback, audit,
undo.

## The context

`setup` gets a `GhostContext`: `data_dir` and `cache_dir` (yours), `store` (a key/value
namespace in Ghost's SQLite), `secrets.get("NAME")` (reads the environment, never logs),
`http()` (an `httpx.AsyncClient`), `log`, `debug_audio(msg)` (silent unless the operator
turned the flag on), `now()`.

## Rules

- Read secrets through `ctx.secrets`, never `os.environ`. Never log them.
- Never execute a write outside the gate. If you can `apply` it, the operator can undo it.
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
