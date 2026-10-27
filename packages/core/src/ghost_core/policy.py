"""Who may do what, decided by the operator, never by the model (ADR 0011, ADR 0024).

* Each source enforces its own target allowlist (repositories, directories) through
  ``Operation.allowed``; anything outside it is denied before autonomy is looked at.
* ``AutonomyPolicy``: how much a source may do on its own, per operation and optionally
  per target. Loaded from ``policy.toml``, which keeps ``[defaults]`` and the levels of
  Ghost's own operations (``[conversations]``, ``[skills]``, ``[departments]``). A
  server's levels live beside its declaration in the department's ``department.json``
  (``ghost_core.departments``), merged over this file for that department's sessions.
  The model cannot read or write any of it, and nothing a server returns reaches it.
"""

from __future__ import annotations

import enum
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any, Literal

from loguru import logger


class PolicyError(PermissionError):
    pass


class Autonomy(enum.StrEnum):
    """How a write operation is handled, least to most autonomous."""

    DENY = "deny"  # not offered to the model at all
    CONFIRM = "confirm"  # propose, ask, wait for a spoken yes, then apply
    ANNOUNCE = "announce"  # apply now, say briefly what happened
    QUIET = "quiet"  # apply now, mention only if asked or if it failed


Risk = Literal["low", "medium", "high"]

# A source's suggested default per operation, used when the operator's file is silent.
RISK_DEFAULTS: dict[str, Autonomy] = {
    "low": Autonomy.ANNOUNCE,
    "medium": Autonomy.CONFIRM,
    "high": Autonomy.CONFIRM,
}

ANY = "*"
# Sections of earlier layouts that this file no longer carries: the ability packages (ADR
# 0022), the servers and toolkits (ADR 0024, now mcp.json and department.json). Ignored
# with a warning so an older file still loads.
RETIRED_SECTIONS = {"abilities": "ADR 0022", "mcp": "ADR 0024: mcp.json", "toolkit": "ADR 0024"}

OpLevels = Mapping[str, Autonomy]


@dataclass(frozen=True)
class AutonomyPolicy:
    """Resolution order: target+op, target+*, source+op, source+*, [defaults], risk.

    File shape::

        [defaults]
        autonomy = "confirm"          # optional; omit to fall back to each op's risk class
        pending_ttl = 3600            # seconds a change at confirm waits (ADR 0030)

        [conversations]               # the agents' own operations (ADR 0014)
        set_persona = "confirm"

        [skills]
        add = "confirm"

        [github]                      # a server's levels may also sit here, by the name
        close = "confirm"             # the gate knows it by; department.json wins
        [github.targets."acme/public-app"]
        close = "deny"
    """

    default: Autonomy | None = None
    skills: Mapping[str, OpLevels] = field(default_factory=dict)
    # ADR 0030: how long a change at ``confirm`` waits on the waiting list; None is the
    # default (``PENDING_TTL``).
    pending_ttl: int | None = None
    # ADR 0031: how Ghost reaches the operator (``ghost_core.routes.RoutePolicy``); None is
    # the default policy. Kept here so it lives in the operator's one file.
    routes: Any = None
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
        pending_ttl: int | None = None
        routes: Any = None
        skills: dict[str, dict[str, Autonomy]] = {}
        targets: dict[str, dict[str, dict[str, Autonomy]]] = {}
        for section, body in raw.items():
            if not isinstance(body, Mapping):
                raise ValueError(f"{source}: [{section}] must be a table")
            if section == "defaults":
                if "autonomy" in body:
                    default = _level(body["autonomy"], f"{source}: defaults.autonomy")
                if "pending_ttl" in body:
                    pending_ttl = _seconds(body["pending_ttl"], f"{source}: defaults.pending_ttl")
                continue
            if section == "routes":
                from ghost_core.routes import RoutePolicy  # here: routes imports the store

                routes = RoutePolicy.parse(body, f"{source}: [routes]")
                continue
            if section in RETIRED_SECTIONS:
                logger.warning(
                    f"{source}: [{section}] is no longer read ({RETIRED_SECTIONS[section]}); "
                    "ignored"
                )
                continue
            ops: dict[str, Autonomy] = {}
            for key, value in body.items():
                if key == "targets":
                    _parse_targets(value, section, section, targets, source)
                    continue
                ops[key] = _level(value, f"{source}: {section}.{key}")
            skills.setdefault(section, {}).update(ops)
        return cls(
            default=default,
            skills=skills,
            targets=targets,
            source=source,
            pending_ttl=pending_ttl,
            routes=routes,
        )

    def resolve(
        self, skill: str, operation: str, target: str = "", risk: Risk = "medium"
    ) -> Autonomy:
        """The level for this source, operation and target."""
        for_target = self.targets.get(skill, {}).get(target.lower(), {})
        for levels in (for_target, self.skills.get(skill, {})):
            if operation in levels:
                return levels[operation]
            if ANY in levels:
                return levels[ANY]
        if self.default is not None:
            return self.default
        return RISK_DEFAULTS[risk]

    @classmethod
    def from_plain(
        cls,
        *,
        default: str | None,
        skills: Mapping[str, Mapping[str, str]],
        targets: Mapping[str, Mapping[str, Mapping[str, str]]],
    ) -> AutonomyPolicy:
        """Build from plain strings (the API's structured update), validating levels."""
        return cls(
            default=_level(default, "defaults.autonomy") if default else None,
            skills={
                s: {op: _level(lv, f"{s}.{op}") for op, lv in ops.items()}
                for s, ops in skills.items()
            },
            targets={
                s: {
                    t.lower(): {op: _level(lv, f"{s}.targets.{t}.{op}") for op, lv in ops.items()}
                    for t, ops in ts.items()
                }
                for s, ts in targets.items()
            },
            source="<api>",
        )

    def to_toml(self) -> str:
        """Render the file the operator owns. Shallow structure, so no TOML writer needed."""
        lines = [
            "# Ghost autonomy policy (ADR 0011). Edited through the desktop app or by hand.",
            "",
        ]
        if self.default is not None or self.pending_ttl is not None:
            lines.append("[defaults]")
            if self.default is not None:
                lines.append(f'autonomy = "{self.default.value}"')
            if self.pending_ttl is not None:
                lines.append(f"pending_ttl = {self.pending_ttl}")
            lines.append("")
        if self.routes is not None:
            lines += [*self.routes.to_toml(), ""]
        for skill in sorted(set(self.skills) | set(self.targets)):
            ops = self.skills.get(skill, {})
            lines.append(f"[{_toml_key(skill)}]")
            for op, lv in ops.items():
                lines.append(f'{_toml_key(op)} = "{lv.value}"')
            lines.append("")
            for target, tops in self.targets.get(skill, {}).items():
                lines.append(f'[{_toml_key(skill)}.targets."{target}"]')
                for op, lv in tops.items():
                    lines.append(f'{_toml_key(op)} = "{lv.value}"')
                lines.append("")
        return "\n".join(lines).rstrip() + "\n"

    def describe(self, skill: str, operations: Mapping[str, Risk]) -> dict[str, Autonomy]:
        """The skill-level resolution for each op; targets may still tighten it."""
        return {op: self.resolve(skill, op, risk=risk) for op, risk in operations.items()}


def _seconds(value: object, where: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 60 <= value <= 7 * 86400:
        raise ValueError(f"{where} must be a whole number of seconds from 60 to 604800")
    return value


def _toml_key(k: str) -> str:
    return k if k.replace("_", "").replace("-", "").isalnum() else f'"{k}"'


def _parse_targets(
    value: object,
    skill: str,
    where: str,
    targets: dict[str, dict[str, dict[str, Autonomy]]],
    source: str,
) -> None:
    if not isinstance(value, Mapping):
        raise ValueError(f"{source}: [{where}.targets] must be a table")
    for tname, tops in value.items():
        if not isinstance(tops, Mapping):
            raise ValueError(f"{source}: [{where}.targets.{tname}] must be a table")
        targets.setdefault(skill, {})[tname.lower()] = {
            op: _level(lv, f"{source}: {where}.targets.{tname}.{op}") for op, lv in tops.items()
        }


def _level(value: object, where: str) -> Autonomy:
    try:
        return Autonomy(str(value))
    except ValueError:
        raise ValueError(
            f"{where}: {value!r} is not one of {', '.join(a.value for a in Autonomy)}"
        ) from None
