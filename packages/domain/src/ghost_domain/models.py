"""Typed proposals. The LLM can only produce these through a skill's ``prepare``.

Every step knows how to invert itself against the snapshot taken before it was
applied, which is what makes "undo that" possible (ADR 0011)."""

from __future__ import annotations

import enum
import uuid
from datetime import UTC, datetime, timedelta
from typing import Annotated, Literal

from pydantic import BaseModel, Field

PROPOSAL_TTL = timedelta(seconds=60)
UNDO_WINDOW = timedelta(minutes=10)
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

    def invert(self, before: Snapshot, marker: str) -> Step | None:
        had = set(before.labels)
        add = tuple(lb for lb in self.remove if lb in had)  # put back what we took off
        remove = tuple(lb for lb in self.add if lb not in had)  # take off what we added
        return SetLabels(add=add, remove=remove) if add or remove else None


class AddComment(BaseModel, frozen=True):
    kind: Literal["add_comment"] = "add_comment"
    body: str = Field(min_length=1, max_length=MAX_COMMENT_CHARS)

    def describe(self) -> str:
        return f"comment: {self.body}"

    def invert(self, before: Snapshot, marker: str) -> Step | None:
        return DeleteComment(marker=marker)


class DeleteComment(BaseModel, frozen=True):
    """Only ever produced by undo: removes the comment carrying ``marker``."""

    kind: Literal["delete_comment"] = "delete_comment"
    marker: str

    def describe(self) -> str:
        return "delete the comment I added"

    def invert(self, before: Snapshot, marker: str) -> Step | None:
        return None  # the body is gone; not undoable


class SetState(BaseModel, frozen=True):
    kind: Literal["set_state"] = "set_state"
    state: Literal["open", "closed"]

    def describe(self) -> str:
        return f"{self.state} the issue"

    def invert(self, before: Snapshot, marker: str) -> Step | None:
        return SetState(state=before.state) if before.state != self.state else None


Step = Annotated[SetLabels | AddComment | DeleteComment | SetState, Field(discriminator="kind")]


class ChangeSet(BaseModel, frozen=True):
    steps: tuple[Step, ...] = Field(min_length=1, max_length=MAX_STEPS)

    def describe(self) -> str:
        return "; ".join(s.describe() for s in self.steps)

    def invert(self, before: Snapshot, marker: str) -> ChangeSet | None:
        """The steps that put ``before`` back, last applied first. None if nothing can be."""
        steps = [inv for st in reversed(self.steps) if (inv := st.invert(before, marker))]
        return ChangeSet(steps=tuple(steps)) if steps else None


class ProposalStatus(enum.StrEnum):
    PENDING = "pending"
    CLAIMED = "claimed"
    CANCELLED = "cancelled"
    EXPIRED = "expired"
    STALE = "stale"
    EXECUTED = "executed"
    UNDONE = "undone"


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
    autonomy: str = "confirm"  # the level the policy resolved to; audit, not control
    undo_of: str | None = None  # set on the proposal that reverses another
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

    def summary(self) -> str:
        """Terse form for the model to paraphrase after an autonomous write."""
        if self.status == "success":
            return f"Applied on {self.target}: {'; '.join(self.applied)}."
        if self.status == "partial":
            return (
                f"Partly applied on {self.target}: {'; '.join(self.applied)}. "
                f"Failed: {self.failed_step} ({self.error})."
            )
        return f"Nothing changed on {self.target}: {self.error}"


def _spoken_snapshot(s: Snapshot | None) -> str:
    if s is None:
        return "unknown state"
    labels = ", ".join(s.labels) if s.labels else "no labels"
    return f"{s.state}, labels {labels}"
