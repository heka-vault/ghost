"""The write model every ability shares.

An ability defines its own ``Target``, ``Snapshot`` and ``Change`` subclasses and a
``Writer`` that applies them. The gate never names a concrete type: it snapshots,
bounds, applies, reads back and audits through these four interfaces, so a
GitHub issue, a local file and a calendar entry get the same protections.

There is no undo machinery. Reversing a change is an ordinary change the model
proposes (reopen the issue, set the labels back), through the same gate and policy.
"""

from __future__ import annotations

import enum
import uuid
from datetime import UTC, datetime, timedelta
from typing import Literal

from pydantic import BaseModel, ConfigDict, Field, SerializeAsAny

PROPOSAL_TTL = timedelta(seconds=60)
MAX_STEPS = 4


def utcnow() -> datetime:
    return datetime.now(UTC)


class Target(BaseModel, frozen=True):
    """Something an ability can write to. Subclass with the identifying fields."""

    def key(self) -> str:
        """Stable identity, e.g. ``github:acme/app#151`` or ``file:/home/me/notes.md``."""
        raise NotImplementedError

    def scope(self) -> str:
        """What a policy target rule matches against, e.g. the repository or the directory.
        Defaults to the key."""
        return self.key()

    def spoken(self) -> str:
        """How Ghost says it aloud."""
        return self.key()

    def mentions(self) -> set[str]:
        """Words an operator would use to name it in a request ("17", "seventeen"). Empty
        means a request can't name it, so a confirm-level change on it always asks."""
        return set()

    def __str__(self) -> str:
        return self.spoken()


class Snapshot(BaseModel, frozen=True):
    """What the target looked like when the proposal was prepared.

    ``version`` is compared again right before execution: if the target changed in
    between, the proposal is stale and is not applied. Updated-at, mtime, ETag."""

    version: str

    def spoken(self) -> str:
        raise NotImplementedError


class Change(BaseModel, frozen=True):
    """One bounded step. Subclass per kind of edit the ability can make."""

    def describe(self) -> str:
        raise NotImplementedError


class ChangeSet(BaseModel, frozen=True):
    model_config = ConfigDict(arbitrary_types_allowed=True)

    steps: tuple[SerializeAsAny[Change], ...] = Field(min_length=1, max_length=MAX_STEPS)

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

    model_config = ConfigDict(arbitrary_types_allowed=True)

    id: str = Field(default_factory=lambda: uuid.uuid4().hex)
    session_id: str
    ability: str
    operation: str
    target: SerializeAsAny[Target]
    changes: ChangeSet
    rationale: str
    snapshot: SerializeAsAny[Snapshot]
    preview: str
    autonomy: str = "confirm"  # the level the policy resolved to; audit, not control
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

    model_config = ConfigDict(arbitrary_types_allowed=True)

    proposal_id: str
    target: SerializeAsAny[Target]
    status: Literal["success", "partial", "failed"]
    applied: tuple[str, ...]
    failed_step: str | None = None
    error: str | None = None
    readback: SerializeAsAny[Snapshot] | None = None

    def spoken(self) -> str:
        now = self.readback.spoken() if self.readback else "unknown state"
        if self.status == "success":
            return f"Done. {self.target} now: {now}."
        if self.status == "partial":
            return (
                f"Partly done on {self.target}. Applied {', '.join(self.applied)}; "
                f"{self.failed_step} failed: {self.error}. It now: {now}."
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
