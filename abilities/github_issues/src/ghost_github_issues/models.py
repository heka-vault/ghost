"""GitHub issue targets, snapshots and changes, plus the repository allowlist."""

from __future__ import annotations

from typing import Literal

from ghost_core import Change, PolicyError, Snapshot, Target
from pydantic import Field

MAX_COMMENT_CHARS = 2000


class IssueTarget(Target, frozen=True):
    owner: str
    repo: str
    number: int

    @property
    def full_name(self) -> str:
        return f"{self.owner}/{self.repo}"

    def key(self) -> str:
        return f"github:{self.full_name}#{self.number}"

    def scope(self) -> str:
        return self.full_name

    def spoken(self) -> str:
        return f"{self.full_name}#{self.number}"


class IssueSnapshot(Snapshot, frozen=True):
    """``version`` is GitHub's ``updated_at``."""

    title: str
    state: Literal["open", "closed"]
    labels: tuple[str, ...]

    def spoken(self) -> str:
        labels = ", ".join(self.labels) if self.labels else "no labels"
        return f"{self.state}, labels {labels}"


class SetLabels(Change, frozen=True):
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

    def invert(self, before: Snapshot, marker: str) -> Change | None:
        assert isinstance(before, IssueSnapshot)
        had = set(before.labels)
        add = tuple(lb for lb in self.remove if lb in had)  # put back what we took off
        remove = tuple(lb for lb in self.add if lb not in had)  # take off what we added
        return SetLabels(add=add, remove=remove) if add or remove else None


class AddComment(Change, frozen=True):
    kind: Literal["add_comment"] = "add_comment"
    body: str = Field(min_length=1, max_length=MAX_COMMENT_CHARS)

    def describe(self) -> str:
        return f"comment: {self.body}"

    def invert(self, before: Snapshot, marker: str) -> Change | None:
        return DeleteComment(marker=marker)


class DeleteComment(Change, frozen=True):
    """Only ever produced by undo: removes the comment carrying ``marker``."""

    kind: Literal["delete_comment"] = "delete_comment"
    marker: str

    def describe(self) -> str:
        return "delete the comment I added"


class SetState(Change, frozen=True):
    kind: Literal["set_state"] = "set_state"
    state: Literal["open", "closed"]

    def describe(self) -> str:
        return f"{self.state} the issue"

    def invert(self, before: Snapshot, marker: str) -> Change | None:
        assert isinstance(before, IssueSnapshot)
        return SetState(state=before.state) if before.state != self.state else None


class RepoPolicy:
    """Repository allowlist. Enforced here and by the PAT's repo selection (ADR 0008)."""

    def __init__(self, allowed: frozenset[str]) -> None:
        self.allowed = allowed  # "owner/repo", lowercase

    @classmethod
    def from_csv(cls, csv: str) -> RepoPolicy:
        return cls(frozenset(s.strip().lower() for s in csv.split(",") if s.strip()))

    def allows(self, full_name: str) -> bool:
        return full_name.lower() in self.allowed

    def allows_target(self, target: Target) -> bool:
        return isinstance(target, IssueTarget) and self.allows(target.full_name)

    def check(self, target: IssueTarget) -> None:
        if not self.allows(target.full_name):
            raise PolicyError(f"{target.full_name} is not an allowed repository")

    def __len__(self) -> int:
        return len(self.allowed)
