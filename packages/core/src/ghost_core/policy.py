"""Who may do what, decided by the operator, never by the model (ADR 0011).

* Each source enforces its own target allowlist (repositories, directories) through
  ``Operation.allowed``; anything outside it is denied before autonomy is looked at.
* ``AutonomyPolicy``: how much a source may do on its own, per operation and
  optionally per target, and the MCP servers Ghost connects to (ADR 0022,
  ``[mcp.<name>]``). Loaded from a TOML file the operator owns. The model cannot read
  or write it, and nothing a server returns reaches it.
"""

from __future__ import annotations

import enum
import tomllib
from collections.abc import Mapping, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import TYPE_CHECKING, Literal

if TYPE_CHECKING:  # ghost_core.mcp imports the scope module, which imports this one
    from ghost_core.mcp import ServerSpec


class PolicyError(PermissionError):
    pass


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
    """Resolution order: target+op, target+*, source+op, source+*, [defaults], risk.

    File shape::

        [mcp.mobbin]                           # an MCP server (ADR 0022, ghost_core.mcp)
        url = "http://proxy.lan:8318/mobbin/mcp"
        headers = { Authorization = "Bearer $MCP_PROXY_KEY" }

        [mcp.github.levels]                    # a server's operations, declared under
        close = "confirm"                      # [mcp.github.writes.<op>], take their
        [mcp.github.targets."acme/public-app"] # levels here, per target as above
        close = "deny"

        [defaults]
        autonomy = "confirm"          # optional; omit to fall back to each op's risk class

        [conversations]                        # the lines' own operations (ADR 0014)
        set_persona = "confirm"
    """

    default: Autonomy | None = None
    skills: Mapping[str, OpLevels] = field(default_factory=dict)
    targets: Mapping[str, Mapping[str, OpLevels]] = field(default_factory=dict)
    mcp: Mapping[str, ServerSpec] = field(default_factory=dict)  # [mcp.<name>] (ADR 0022)
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
        mcp: dict[str, ServerSpec] = {}
        for section, body in raw.items():
            if not isinstance(body, Mapping):
                raise ValueError(f"{source}: [{section}] must be a table")
            if section == "defaults":
                if "autonomy" in body:
                    default = _level(body["autonomy"], f"{source}: defaults.autonomy")
                continue
            if section == "mcp":
                from ghost_core.mcp import parse_servers

                for sname, sbody in body.items():
                    if not isinstance(sbody, Mapping):
                        continue  # parse_servers reports it
                    if "levels" in sbody:
                        lv = sbody["levels"]
                        if not isinstance(lv, Mapping):
                            raise ValueError(f"{source}: [mcp.{sname}.levels] must be a table")
                        skills.setdefault(sname, {}).update(
                            {
                                op: _level(v, f"{source}: mcp.{sname}.levels.{op}")
                                for op, v in lv.items()
                            }
                        )
                    if "targets" in sbody:
                        _parse_targets(sbody["targets"], sname, f"mcp.{sname}", targets, source)
                mcp = parse_servers(body, source)
                continue
            if section == "abilities":  # the retired ability packages (ADR 0022)
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
            mcp=mcp,
            source=source,
        )

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

    @classmethod
    def from_plain(
        cls,
        *,
        default: str | None,
        skills: Mapping[str, Mapping[str, str]],
        targets: Mapping[str, Mapping[str, Mapping[str, str]]],
        mcp: Mapping[str, ServerSpec] | None = None,
    ) -> AutonomyPolicy:
        """Build from plain strings (the API's structured update), validating levels.
        ``mcp`` is carried over from the file as is: the structured update doesn't edit
        servers, and rendering the file must not lose them."""
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
            mcp=dict(mcp or {}),
            source="<api>",
        )

    def to_toml(self) -> str:
        """Render the file the operator owns. Shallow structure, so no TOML writer needed."""
        lines = [
            "# Ghost autonomy policy (ADR 0011). Edited through the desktop app or by hand.",
            "",
        ]
        if self.default is not None:
            lines += ["[defaults]", f'autonomy = "{self.default.value}"', ""]
        if self.mcp:
            from ghost_core.mcp import render_server

            for name, spec in self.mcp.items():
                lines += render_server(name, spec)
                ops = self.skills.get(name, {})
                if ops:
                    lines.append(f"[mcp.{_toml_key(name)}.levels]")
                    lines += [f'{_toml_key(op)} = "{lv.value}"' for op, lv in ops.items()]
                    lines.append("")
                for target, tops in self.targets.get(name, {}).items():
                    lines.append(f'[mcp.{_toml_key(name)}.targets."{target}"]')
                    lines += [f'{_toml_key(op)} = "{lv.value}"' for op, lv in tops.items()]
                    lines.append("")
        for skill in sorted((set(self.skills) | set(self.targets)) - set(self.mcp)):
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


def _toml_key(k: str) -> str:
    return k if k.replace("_", "").replace("-", "").isalnum() else f'"{k}"'


def _toml_list(items: Sequence[str]) -> str:
    return "[" + ", ".join(f'"{i}"' for i in items) + "]"


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
