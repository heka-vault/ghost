"""The ability API: what an installable package contributes to Ghost (ADR 0012).

An ability is a package with an ``Ability`` object exported through the
``ghost.abilities`` entry point group (or dropped into
``~/.config/ghost/abilities/<name>/``). Ghost instantiates its settings from the
environment, calls ``setup`` with a ``GhostContext``, and asks the returned instance
for its ``Contributions``. Every field is optional: a read-only ability declares a
prompt paragraph and some read tools and is done.
"""

from __future__ import annotations

import enum
from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any, Literal, Protocol

from ghost_core.models import ChangeSet, Target
from ghost_core.policy import Risk
from ghost_core.writer import Writer

ToolResult = dict[str, Any]
ToolHandler = Callable[[dict[str, Any]], Awaitable[ToolResult]]


@dataclass(frozen=True)
class ReadTool:
    """A typed, read-only function the model may call directly."""

    name: str
    description: str
    properties: dict[str, Any]
    required: list[str]
    handler: ToolHandler


@dataclass(frozen=True)
class Prepared:
    """What an operation hands to the gate: a target, a typed change set, and why."""

    target: Target
    changes: ChangeSet
    rationale: str = ""


def _allow_all(_target: Target) -> bool:
    return True


@dataclass(frozen=True)
class Operation:
    """A named write. The model supplies ``properties``; ``prepare`` validates them into
    a ``Prepared``; the gate applies it through ``writer`` at the autonomy level the
    operator's policy gives this operation. ``risk`` is only the suggested default.
    ``allowed`` is the ability's own allowlist (repositories, directories, calendars);
    the gate checks it before anything else."""

    name: str
    description: str
    properties: dict[str, Any]
    required: list[str]
    prepare: Callable[[dict[str, Any]], Awaitable[Prepared]]
    writer: Writer
    risk: Risk = "medium"
    allowed: Callable[[Target], bool] = _allow_all
    # Words in the operator's request that name this operation ("close", "shut"). When a
    # confirm-level proposal follows a request that plainly asks for it on that target,
    # the request is the yes (ghost_core.request). Empty: always ask.
    asked_by: tuple[str, ...] = ()


# --- per-turn hooks ---------------------------------------------------------------


@dataclass(frozen=True)
class UserTurn:
    """What the operator just said, with the context an interceptor needs."""

    text: str
    pending: bool  # a confirm-level change is waiting for a yes
    last_assistant: str  # Ghost's previous reply


@dataclass(frozen=True)
class Handled:
    """An interceptor took the turn. The model never sees it."""

    say: str | None = None
    action: Literal["sleep", "cancel"] | None = None


class TurnInterceptor(Protocol):
    """Looks at a user turn before the model does. Return ``Handled`` to take it, None
    to pass. Runs inside a session only; never on the passive path."""

    name: str

    async def on_user_turn(self, turn: UserTurn) -> Handled | None: ...


class Slot(enum.StrEnum):
    """Where in the voice pipeline an ability's Pipecat processors are spliced in."""

    AUDIO_IN = "audio_in"  # raw mic frames in session
    AFTER_STT = "after_stt"  # transcription frames
    BEFORE_LLM = "before_llm"  # aggregated user turn
    AFTER_LLM = "after_llm"  # model text and tool frames
    BEFORE_TTS = "before_tts"  # sentences to speak
    AUDIO_OUT = "audio_out"  # TTS audio


ProcessorFactory = Callable[[], Any]  # -> pipecat FrameProcessor; one per session


# --- lifecycle --------------------------------------------------------------------


@dataclass(frozen=True)
class Event:
    kind: Literal["wake", "session_start", "session_end", "outcome", "error"]
    data: dict[str, Any] = field(default_factory=dict)


EventListener = Callable[[Event], Awaitable[None]]


@dataclass(frozen=True)
class CliCommand:
    """``ghostd <ability> <name> [args]``."""

    name: str
    description: str
    run: Callable[[list[str]], int]


# --- the bundle -------------------------------------------------------------------


@dataclass
class Contributions:
    prompt: str = ""
    read_tools: list[ReadTool] = field(default_factory=list)
    operations: list[Operation] = field(default_factory=list)
    interceptors: list[TurnInterceptor] = field(default_factory=list)
    processors: dict[Slot, list[ProcessorFactory]] = field(default_factory=dict)
    listeners: list[EventListener] = field(default_factory=list)
    commands: list[CliCommand] = field(default_factory=list)
    # Scope (ADR 0019). ``scope_kind`` says what a thread can be scoped to for this
    # ability ("repository", "directory", "workspace"); ``scope_items`` returns the whole
    # allowlist, each item the string ``Target.scope()`` yields for it. A scoped ability
    # gets an ``ActiveScope`` as the ``_scope`` argument of every read tool and
    # ``prepare`` call (``ghost_core.scope.scope_of(args)``), and may give
    # ``scoped_prompt`` to narrow its prompt paragraph to the active thread. Abilities
    # that declare no scope kind are unaffected.
    scope_kind: str | None = None
    scope_items: Callable[[], list[str]] | None = None
    scoped_prompt: Callable[[Any], str] | None = None  # (ActiveScope | None) -> str


class AbilityInstance(Protocol):
    def contributions(self) -> Contributions: ...

    async def aclose(self) -> None: ...


@dataclass
class SimpleInstance:
    """For abilities that have no state to close beyond a callback."""

    _contributions: Contributions
    _aclose: Callable[[], Awaitable[None]] | None = None

    def contributions(self) -> Contributions:
        return self._contributions

    async def aclose(self) -> None:
        if self._aclose:
            await self._aclose()


@dataclass(frozen=True)
class Ability:
    """The exported object. ``config`` is a pydantic-settings class Ghost instantiates
    from the environment (use ``env_prefix="GHOST_<NAME>_"``), or None. ``passive``
    marks abilities that may run before the wake phrase; the operator must also list
    them in the policy file, because they hear idle audio."""

    name: str
    version: str
    description: str
    setup: Callable[[Any, Any], Awaitable[AbilityInstance]]  # (GhostContext, settings)
    config: type | None = None
    passive: bool = False
