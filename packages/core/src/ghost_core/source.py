"""What a source of tools gives Ghost: a prompt paragraph, typed read tools, write
operations for the gate, and the pipeline hooks Ghost's own parts use.

Sources are core: the lines (ADR 0014), Ghost's fixed intents, and every MCP server the
operator declares (ADR 0022). There is no package mechanism any more; a ``ToolSource`` is
built in code from what a server or a core module offers.
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
    ``allowed`` is the source's own allowlist (repositories, directories, calendars);
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
    # hang_up ends the session; hold keeps it while the operator talks to someone else
    # (ADR 0021); cancel drops a pending change; quiet is do-not-disturb until ``until``
    # (ADR 0031), the operator's own words, never the model's.
    action: Literal["hang_up", "hold", "cancel", "quiet"] | None = None
    until: Any = None  # a datetime, for quiet


class TurnInterceptor(Protocol):
    """Looks at a user turn before the model does. Return ``Handled`` to take it, None
    to pass. Runs inside a session only; never on the passive path."""

    name: str

    async def on_user_turn(self, turn: UserTurn) -> Handled | None: ...


class Slot(enum.StrEnum):
    """Where in the voice pipeline a processor sits (Ghost's own, by slot)."""

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
    kind: Literal["wake", "session_start", "session_end", "outcome", "error", "quiet"]
    data: dict[str, Any] = field(default_factory=dict)


EventListener = Callable[[Event], Awaitable[None]]


@dataclass
class ToolSource:
    """One source of tools, as the pipeline and the gate take it. ``name`` is the key the
    policy, the gate registry and the audit use (``github.close``; a department's own
    server is ``marketing/figma``, ADR 0024). ``scope_kind`` and ``scope_items`` say what
    the source's allowlist is (ADR 0019); a scoped source gets an ``ActiveScope`` as the
    ``_scope`` argument of every read tool and ``prepare`` call, which names the
    departments that cover an item outside it, and may give ``scoped_prompt``."""

    name: str
    prompt: str = ""
    read_tools: list[ReadTool] = field(default_factory=list)
    operations: list[Operation] = field(default_factory=list)
    interceptors: list[TurnInterceptor] = field(default_factory=list)
    processors: dict[Slot, list[ProcessorFactory]] = field(default_factory=dict)
    listeners: list[EventListener] = field(default_factory=list)
    scope_kind: str | None = None
    scope_items: Callable[[], list[str]] | None = None
    scoped_prompt: Callable[[Any], str] | None = None  # ActiveScope | None -> paragraph
