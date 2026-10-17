"""Who may do what, decided by the operator, never by the model (ADR 0011).

Two layers:

* ``RepoPolicy``: the target allowlist. Enforced here and by the PAT's repo
  selection (ADR 0008). Anything outside it is denied before autonomy is even
  looked at.
* ``AutonomyPolicy``: how much a skill may do on its own, per operation and
  optionally per target. Loaded from a TOML file the operator owns. The model
  cannot read or write it, and issue text never reaches it.
"""

from __future__ import annotations

import enum
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Literal

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


class Autonomy(enum.StrEnum):
    """How a write operation is handled, least to most autonomous."""

    DENY = "deny"  # not offered to the model at all
    CONFIRM = "confirm"  # propose, ask, wait for a spoken yes, then apply
    ANNOUNCE = "announce"  # apply now, say briefly what happened
    QUIET = "quiet"  # apply now, mention only if asked or if it failed


Risk = Literal["low", "medium", "high"]

# A skill's suggested default per operation, used when the operator's file is silent.
RISK_DEFAULTS: dict[str, Autonomy] = {
    "low": Autonomy.ANNOUNCE,
    "medium": Autonomy.CONFIRM,
    "high": Autonomy.CONFIRM,
}

ANY = "*"

OpLevels = Mapping[str, Autonomy]


@dataclass(frozen=True)
class AutonomyPolicy:
    """Resolution order: target+op, target+*, skill+op, skill+*, [defaults], risk class.

    File shape::

        [defaults]
        autonomy = "confirm"          # optional; omit to fall back to each op's risk class

        [github_issues]
        set_labels = "announce"
        close = "confirm"

        [github_issues.targets."acme/public-app"]
        close = "deny"
    """

    default: Autonomy | None = None
    skills: Mapping[str, OpLevels] = field(default_factory=dict)
    targets: Mapping[str, Mapping[str, OpLevels]] = field(default_factory=dict)
    source: str = "<none>"

    @classmethod
    def load(cls, path: str | Path) -> AutonomyPolicy:
        """A missing file is an empty policy: every operation falls back to its risk class."""
        p = Path(path)
        if not p.exists():
            return cls(source=f"{p} (missing)")
        return cls.parse(p.read_text(), source=str(p))

    @classmethod
    def parse(cls, text: str, source: str = "<string>") -> AutonomyPolicy:
        try:
            raw = tomllib.loads(text)
        except tomllib.TOMLDecodeError as e:
            raise ValueError(f"{source}: {e}") from None
        default: Autonomy | None = None
        skills: dict[str, dict[str, Autonomy]] = {}
        targets: dict[str, dict[str, dict[str, Autonomy]]] = {}
        for section, body in raw.items():
            if not isinstance(body, Mapping):
                raise ValueError(f"{source}: [{section}] must be a table")
            if section == "defaults":
                if "autonomy" in body:
                    default = _level(body["autonomy"], f"{source}: defaults.autonomy")
                continue
            if section == "abilities":  # reserved for the loader; not a skill
                continue
            ops: dict[str, Autonomy] = {}
            for key, value in body.items():
                if key == "targets":
                    if not isinstance(value, Mapping):
                        raise ValueError(f"{source}: [{section}.targets] must be a table")
                    for tname, tops in value.items():
                        if not isinstance(tops, Mapping):
                            raise ValueError(
                                f"{source}: [{section}.targets.{tname}] must be a table"
                            )
                        targets.setdefault(section, {})[tname.lower()] = {
                            op: _level(lv, f"{source}: {section}.targets.{tname}.{op}")
                            for op, lv in tops.items()
                        }
                    continue
                ops[key] = _level(value, f"{source}: {section}.{key}")
            skills[section] = ops
        return cls(default=default, skills=skills, targets=targets, source=source)

    def resolve(
        self, skill: str, operation: str, target: str = "", risk: Risk = "medium"
    ) -> Autonomy:
        for_target = self.targets.get(skill, {}).get(target.lower(), {})
        for levels in (for_target, self.skills.get(skill, {})):
            if operation in levels:
                return levels[operation]
            if ANY in levels:
                return levels[ANY]
        if self.default is not None:
            return self.default
        return RISK_DEFAULTS[risk]

    def describe(self, skill: str, operations: Mapping[str, Risk]) -> dict[str, Autonomy]:
        """The skill-level resolution for each op; targets may still tighten it."""
        return {op: self.resolve(skill, op, risk=risk) for op, risk in operations.items()}


def _level(value: object, where: str) -> Autonomy:
    try:
        return Autonomy(str(value))
    except ValueError:
        raise ValueError(
            f"{where}: {value!r} is not one of {', '.join(a.value for a in Autonomy)}"
        ) from None
