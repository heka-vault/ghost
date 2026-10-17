"""Repository allowlist. Enforced here and by the PAT's repo selection (ADR 0008)."""

from __future__ import annotations

from dataclasses import dataclass

from ghost_domain.models import Target


class PolicyError(PermissionError):
    pass


@dataclass(frozen=True)
class RepoPolicy:
    allowed: frozenset[str]  # "owner/repo", lowercase

    @classmethod
    def from_csv(cls, csv: str) -> RepoPolicy:
        return cls(frozenset(s.strip().lower() for s in csv.split(",") if s.strip()))

    def allows(self, full_name: str) -> bool:
        return full_name.lower() in self.allowed

    def check(self, target: Target) -> None:
        if not self.allows(target.full_name):
            raise PolicyError(f"{target.full_name} is not an allowed repository")

    def __len__(self) -> int:
        return len(self.allowed)
