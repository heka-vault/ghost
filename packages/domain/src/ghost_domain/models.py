"""Typed proposals. The LLM can only produce these through a skill's ``prepare``."""

from __future__ import annotations

import enum
import uuid
from datetime import UTC, datetime, timedelta
from typing import Annotated, Literal

from pydantic import BaseModel, Field

PROPOSAL_TTL = timedelta(seconds=60)
MAX_STEPS = 4
MAX_COMMENT_CHARS = 2000


def utcnow() -> datetime:
    return datetime.now(UTC)


class Target(BaseModel, frozen=True):
    """A GitHub issue, fully qualified."""

    owner: str
    repo: str
    number: int

    @property
    def full_name(self) -> str:
        return f"{self.owner}/{self.repo}"

    def __str__(self) -> str:
        return f"{self.full_name}#{self.number}"


class Snapshot(BaseModel, frozen=True):
    """What the issue looked like when the proposal was prepared.

    ``updated_at`` is compared again right before execution: if the issue changed
    in between, the proposal is stale and is not applied.
    """

    title: str
    state: Literal["open", "closed"]
    labels: tuple[str, ...]
    updated_at: str


class SetLabels(BaseModel, frozen=True):
    kind: Literal["set_labels"] = "set_labels"
    add: tuple[str, ...] = ()
    remove: tuple[str, ...] = ()

    def describe(self) -> str:
        parts = []
        if self.add:
            parts.append("add " + ", ".join(self.add))
        if self.remove:
            parts.append("remove " + ", ".join(self.remove))
        return "labels: " + "; ".join(parts) if parts else "labels: no change"


class AddComment(BaseModel, frozen=True):
    kind: Literal["add_comment"] = "add_comment"
    body: str = Field(min_length=1, max_length=MAX_COMMENT_CHARS)

    def describe(self) -> str:
        return f"comment: {self.body}"


class SetState(BaseModel, frozen=True):
    kind: Literal["set_state"] = "set_state"
    state: Literal["open", "closed"]

    def describe(self) -> str:
        return f"{self.state} the issue"


Step = Annotated[SetLabels | AddComment | SetState, Field(discriminator="kind")]


class ChangeSet(BaseModel, frozen=True):
    steps: tuple[Step, ...] = Field(min_length=1, max_length=MAX_STEPS)

    def describe(self) -> str:
        return "; ".join(s.describe() for s in self.steps)


class ProposalStatus(enum.StrEnum):
    PENDING = "pending"
    CLAIMED = "claimed"
    CANCELLED = "cancelled"
    EXPIRED = "expired"
    STALE = "stale"
    EXECUTED = "executed"


class Proposal(BaseModel, frozen=True):
    """Immutable once stored. Only ``status`` changes, through the store."""

    id: str = Field(default_factory=lambda: uuid.uuid4().hex)
    session_id: str
    skill: str
    operation: str
    target: Target
    changes: ChangeSet
    rationale: str
    snapshot: Snapshot
    preview: str
    created_at: datetime = Field(default_factory=utcnow)
    expires_at: datetime = Field(default_factory=lambda: utcnow() + PROPOSAL_TTL)
    status: ProposalStatus = ProposalStatus.PENDING

    @property
    def idempotency_key(self) -> str:
        return f"ghost:{self.id}"

    def is_expired(self, now: datetime | None = None) -> bool:
        return (now or utcnow()) >= self.expires_at


class Outcome(BaseModel, frozen=True):
    """The verified result the agent speaks. Never a guess."""

    proposal_id: str
    target: Target
    status: Literal["success", "partial", "failed"]
    applied: tuple[str, ...]
    failed_step: str | None = None
    error: str | None = None
    readback: Snapshot | None = None

    def spoken(self) -> str:
        if self.status == "success":
            return f"Done. {self.target} now: {_spoken_snapshot(self.readback)}."
        if self.status == "partial":
            return (
                f"Partly done on {self.target}. Applied {', '.join(self.applied)}; "
                f"{self.failed_step} failed: {self.error}. "
                f"It now: {_spoken_snapshot(self.readback)}."
            )
        return f"Nothing changed on {self.target}. {self.error}"


def _spoken_snapshot(s: Snapshot | None) -> str:
    if s is None:
        return "unknown state"
    labels = ", ".join(s.labels) if s.labels else "no labels"
    return f"{s.state}, labels {labels}"
