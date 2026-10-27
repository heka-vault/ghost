"""The operator's file: what Ghost may do, how long a change waits for a yes, and how Ghost
reaches the operator (ADR 0011, ADR 0030, ADR 0031, ADR 0034).

Everything runs unless the operator's permissions say otherwise (``ghost_core.permissions``):
``[permissions]`` holds the mode and the ``block``, ``ask``, ``quiet`` and ``allow`` lists.
A department's ``department.json`` may add ``block``, ``ask`` and ``quiet`` rules for its own
sessions (``with_rules``), never ``allow``. The model cannot read or write any of it, and
nothing an app returns reaches it.

The older layout, a table per source with a level per operation (``[conversations]
delete = "confirm"``, ``[github.targets."acme/app"] close = "deny"``), still reads: each
entry becomes a rule in the list its level names.
"""

from __future__ import annotations

import re
import tomllib
from collections.abc import Mapping
from dataclasses import dataclass, field, replace
from pathlib import Path
from typing import Any, Literal

from loguru import logger

from ghost_core.levels import Autonomy
from ghost_core.permissions import (
    LISTS,
    Condition,
    Mode,
    Permissions,
    Rule,
)


class PolicyError(PermissionError):
    pass


Risk = Literal["low", "medium", "high"]  # a source's own note; the level is the operator's

ANY = "*"
# Sections of earlier layouts that this file no longer carries: the ability packages (ADR
# 0022), the servers and toolkits (ADR 0024, then apps.json, ADR 0034). Ignored with a
# warning so an older file still loads.
RETIRED_SECTIONS = {"abilities": "ADR 0022", "mcp": "ADR 0034: apps.json", "toolkit": "ADR 0024"}
_LEGACY_LIST = {
    Autonomy.DENY: "block",
    Autonomy.CONFIRM: "ask",
    Autonomy.QUIET: "quiet",
    Autonomy.ANNOUNCE: "allow",
}
_LEGACY_MODE = {
    Autonomy.DENY: Mode.READ_ONLY,
    Autonomy.CONFIRM: Mode.ASK,
    Autonomy.ANNOUNCE: Mode.OPEN,
    Autonomy.QUIET: Mode.OPEN,
}


@dataclass(frozen=True)
class AutonomyPolicy:
    """File shape::

    [permissions]
    mode = "open"                 # open | ask | read-only: a change no rule names
    block = ["gmail.delete_*"]
    ask = ["github.issue_write(state=closed)"]
    quiet = ["github.add_issue_labels"]
    allow = []                    # exceptions, when mode is ask or read-only

    [defaults]
    pending_ttl = 3600            # seconds a change Ghost asked about waits (ADR 0030)

    [routes]                      # ADR 0031
    """

    permissions: Permissions = field(default_factory=Permissions)
    # ADR 0030: how long a change at ``confirm`` waits on the waiting list; None is the
    # default (``PENDING_TTL``).
    pending_ttl: int | None = None
    # ADR 0031: how Ghost reaches the operator (``ghost_core.routes.RoutePolicy``); None is
    # the default policy. Kept here so it lives in the operator's one file.
    routes: Any = None
    source: str = "<none>"

    @classmethod
    def load(cls, path: str | Path) -> AutonomyPolicy:
        """A missing file is an empty policy: everything runs and is announced."""
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
        permissions = Permissions()
        pending_ttl: int | None = None
        routes: Any = None
        legacy: dict[str, list[Rule]] = {k: [] for k in LISTS}
        legacy_mode: Mode | None = None
        for section, body in raw.items():
            if not isinstance(body, Mapping):
                raise ValueError(f"{source}: [{section}] must be a table")
            if section == "permissions":
                permissions = Permissions.parse(body, f"{source}: [permissions]")
                continue
            if section == "defaults":
                if "autonomy" in body:
                    level = _level(body["autonomy"], f"{source}: defaults.autonomy")
                    legacy_mode = _LEGACY_MODE[level]
                    if level is Autonomy.QUIET:
                        legacy["quiet"].append(Rule(app=ANY))
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
            _legacy_rules(section, body, legacy, source)
        if legacy_mode is not None and "permissions" not in raw:
            permissions = replace(permissions, mode=legacy_mode)
        if any(legacy.values()):
            permissions = Permissions(
                mode=permissions.mode,
                **{k: (*legacy[k], *getattr(permissions, k)) for k in LISTS},
            )
        return cls(
            permissions=permissions,
            source=source,
            pending_ttl=pending_ttl,
            routes=routes,
        )

    def resolve(
        self, source: str, operation: str, args: Mapping[str, Any] | None = None
    ) -> Autonomy:
        """The level for this change. ``args`` are the call's arguments, for the rules
        with argument patterns; None decides on the name alone."""
        return self.permissions.resolve(source, operation, args)

    def with_rules(self, extra: Permissions, where: str = "") -> AutonomyPolicy:
        """This policy with a department's rules added (they only ever add care)."""
        if not any(getattr(extra, k) for k in LISTS):
            return self
        return replace(
            self,
            permissions=self.permissions.merged(extra),
            source=f"{self.source} + {where}" if where else self.source,
        )

    def with_permissions(self, permissions: Permissions) -> AutonomyPolicy:
        return replace(self, permissions=permissions, source="<api>")

    def to_toml(self) -> str:
        """Render the file the operator owns. Shallow structure, so no TOML writer needed."""
        lines = [
            "# Ghost's permissions (ADR 0034). Edited through the desktop app or by hand.",
            "# Everything runs unless a rule below blocks it or asks first.",
            "",
            *self.permissions.to_toml(),
            "",
        ]
        if self.pending_ttl is not None:
            lines += ["[defaults]", f"pending_ttl = {self.pending_ttl}", ""]
        if self.routes is not None:
            lines += [*self.routes.to_toml(), ""]
        return "\n".join(lines).rstrip() + "\n"


_HEADER = re.compile(r"^\s*\[\s*([^\[\]]+?)\s*\]\s*(#.*)?$")


def write_permissions(text: str, permissions: Permissions) -> str:
    """``text`` with its ``[permissions]`` table replaced by ``permissions``, everything
    else kept as the operator wrote it (comments, ``[routes]``, ``[defaults]``). The older
    per-source tables are folded in and dropped, since their rules are in the lists now,
    and so is ``[defaults] autonomy``, which the mode replaces."""
    keep_top = {"defaults", "routes", "permissions", *RETIRED_SECTIONS}
    lines = text.splitlines()
    out: list[str] = []
    section: str | None = None
    placed = False
    rendered = permissions.to_toml()
    for line in lines:
        m = _HEADER.match(line)
        if m:
            section = m.group(1).strip().strip('"')
            top = section.split(".", 1)[0].strip('"')
            if top == "permissions":
                if not placed:
                    out.extend(rendered)
                    placed = True
                continue
            if top not in keep_top:
                continue
            out.append(line)
            continue
        if section is None:
            out.append(line)
            continue
        top = section.split(".", 1)[0].strip('"')
        if top == "permissions" or top not in keep_top:
            continue
        if top == "defaults" and re.match(r"^\s*autonomy\s*=", line):
            continue
        out.append(line)
    if not placed:
        # After the opening comments, before the first table.
        at = next((i for i, ln in enumerate(out) if _HEADER.match(ln)), len(out))
        block = [*rendered, ""]
        if at and out[at - 1].strip():
            block = ["", *block]
        out[at:at] = block
    result = "\n".join(out).rstrip() + "\n"
    AutonomyPolicy.parse(result, source="rendered")  # never write a file that won't read
    return result


def write_routes(text: str, routes: Any) -> str:
    """``text`` with its ``[routes]`` table (and ``[routes.urgency]``) replaced by
    ``routes`` (``ghost_core.routes.RoutePolicy``; None takes it out), where it was, or at
    the end. Everything else stays as the operator wrote it, comments included: the
    comments just above the next table are that table's, so they stay too."""
    rendered: list[str] = routes.to_toml() if routes is not None else []
    out: list[str] = []
    inside = False
    placed = False
    dropped: list[str] = []

    def leave() -> None:
        tail: list[str] = []
        while dropped and (not dropped[-1].strip() or dropped[-1].lstrip().startswith("#")):
            tail.insert(0, dropped.pop())
        dropped.clear()
        if out and out[-1].strip() and tail[:1] != [""]:
            tail.insert(0, "")
        out.extend(tail)

    for line in text.splitlines():
        m = _HEADER.match(line)
        if m:
            top = m.group(1).strip().strip('"').split(".", 1)[0].strip('"')
            if top == "routes":
                if not placed:
                    out.extend(rendered)
                    placed = True
                inside = True
                continue
            if inside:
                leave()
                inside = False
        if inside:
            dropped.append(line)
        else:
            out.append(line)
    if inside:
        leave()
    if not placed and rendered:
        while out and not out[-1].strip():
            out.pop()
        out += ["", *rendered] if out else rendered
    result = "\n".join(out).rstrip() + "\n"
    AutonomyPolicy.parse(result, source="rendered")  # never write a file that won't read
    return result


def _legacy_rules(
    section: str, body: Mapping[str, Any], out: dict[str, list[Rule]], source: str
) -> None:
    """``[source] op = "level"`` and ``[source.targets."t"] op = "level"`` as rules."""
    for key, value in body.items():
        if key == "targets":
            if not isinstance(value, Mapping):
                raise ValueError(f"{source}: [{section}.targets] must be a table")
            for tname, tops in value.items():
                if not isinstance(tops, Mapping):
                    raise ValueError(f"{source}: [{section}.targets.{tname}] must be a table")
                for op, lv in tops.items():
                    level = _level(lv, f"{source}: {section}.targets.{tname}.{op}")
                    cond = (Condition("target", str(tname)),)
                    out[_LEGACY_LIST[level]].append(Rule(section, str(op), cond))
            continue
        level = _level(value, f"{source}: {section}.{key}")
        out[_LEGACY_LIST[level]].append(Rule(section, str(key)))


def _seconds(value: object, where: str) -> int:
    if isinstance(value, bool) or not isinstance(value, int) or not 60 <= value <= 7 * 86400:
        raise ValueError(f"{where} must be a whole number of seconds from 60 to 604800")
    return value


def _level(value: object, where: str) -> Autonomy:
    try:
        return Autonomy(str(value))
    except ValueError:
        raise ValueError(
            f"{where}: {value!r} is not one of {', '.join(a.value for a in Autonomy)}"
        ) from None


__all__ = ["ANY", "Autonomy", "AutonomyPolicy", "Mode", "Permissions", "PolicyError", "Risk"]
