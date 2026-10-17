"""The skill interface.

A skill declares:

* ``read_tools``: typed, read-only functions the LLM may call directly.
* ``prepare_ops``: named write operations. The LLM calls ``prepare`` with one of
  these; the skill turns the arguments into a typed ``ChangeSet`` and the domain
  gate takes it from there. A skill never executes a write.
* ``prompt_fragment``: what the system prompt says about this skill.
"""

from __future__ import annotations

from collections.abc import Awaitable, Callable
from dataclasses import dataclass, field
from typing import Any, Protocol

from ghost_domain import ChangeSet, Target

ToolResult = dict[str, Any]
ToolHandler = Callable[[dict[str, Any]], Awaitable[ToolResult]]


@dataclass(frozen=True)
class ReadTool:
    name: str
    description: str
    properties: dict[str, Any]
    required: list[str]
    handler: ToolHandler


@dataclass(frozen=True)
class PrepareRequest:
    """What a skill hands to the gate: a target, a typed change set, and why."""

    operation: str
    target: Target
    changes: ChangeSet
    rationale: str


@dataclass(frozen=True)
class PrepareOp:
    name: str
    description: str
    properties: dict[str, Any]
    required: list[str] = field(default_factory=list)


class Skill(Protocol):
    name: str

    @property
    def prompt_fragment(self) -> str: ...

    @property
    def read_tools(self) -> list[ReadTool]: ...

    @property
    def prepare_ops(self) -> list[PrepareOp]: ...

    async def prepare(self, operation: str, args: dict[str, Any]) -> PrepareRequest:
        """Validate ``args`` and build a typed request. Raises ``ValueError`` on bad input."""
        ...

    async def aclose(self) -> None: ...
