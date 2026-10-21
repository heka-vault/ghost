"""Toolkits (ADR 0023): named bundles of permissions a department is given.

A toolkit grants servers (optionally only some of their reads, some of their declared
writes, some of their scope items, at levels no looser than the policy file's) and skills.
It lives in the policy file and is designed in the desktop; the file is the ceiling and a
toolkit only narrows it. A department's reach is the union of its toolkits.

File shape::

    [toolkit.finance]
    description = "The books and the invoices."
    skills = ["month-close", "invoicing"]

    [toolkit.finance.servers.github_heka]
    reads = ["issue_read", "list_issues"]     # optional: bare tool names; else every read
    writes = ["comment", "set_labels"]        # optional: operations; else every declared one
    items = ["heka-vault/ghost"]              # optional: a subset of the server's scope items
    levels = { set_labels = "confirm" }       # optional: tighter than the file, never looser
"""

from __future__ import annotations

import json
import re
from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any

NAME_RE = re.compile(r"^[a-z0-9][a-z0-9_-]{0,39}$")  # toolkit and server names
LEVEL_RANK = {"deny": 0, "confirm": 1, "announce": 2, "quiet": 3}


@dataclass(frozen=True)
class Grant:
    """One server inside a toolkit."""

    server: str
    reads: tuple[str, ...] | None = None
    writes: tuple[str, ...] | None = None
    items: tuple[str, ...] | None = None
    levels: Mapping[str, str] = field(default_factory=dict)

    def allows_read(self, tool: str) -> bool:
        return self.reads is None or tool in self.reads

    def allows_write(self, operation: str) -> bool:
        return self.writes is None or operation in self.writes


@dataclass(frozen=True)
class ToolkitSpec:
    name: str
    description: str = ""
    grants: Mapping[str, Grant] = field(default_factory=dict)  # by server name
    skills: tuple[str, ...] = ()

    def grant(self, server: str) -> Grant | None:
        return self.grants.get(server)


def tighter(a: str, b: str) -> str:
    """The more cautious of two levels."""
    return a if LEVEL_RANK.get(a, 0) <= LEVEL_RANK.get(b, 0) else b


def _str_list(value: object, where: str) -> tuple[str, ...]:
    if not isinstance(value, list) or not all(isinstance(a, str) for a in value):
        raise ValueError(f"{where} must be a list of strings")
    return tuple(value)


def _parse_grant(toolkit: str, server: str, body: object, source: str) -> Grant:
    where = f"{source}: [toolkit.{toolkit}.servers.{server}]"
    if not isinstance(body, Mapping):
        raise ValueError(f"{where} must be a table")
    unknown = sorted(set(body) - {"reads", "writes", "items", "levels"})
    if unknown:
        raise ValueError(f"{where} has unknown key(s): {', '.join(unknown)}")
    levels = body.get("levels", {})
    if not isinstance(levels, Mapping) or not all(
        isinstance(k, str) and isinstance(v, str) and v in LEVEL_RANK for k, v in levels.items()
    ):
        raise ValueError(f"{where}.levels must map operations to deny, confirm, announce or quiet")
    return Grant(
        server=server,
        reads=_str_list(body["reads"], f"{where}.reads") if "reads" in body else None,
        writes=_str_list(body["writes"], f"{where}.writes") if "writes" in body else None,
        items=_str_list(body["items"], f"{where}.items") if "items" in body else None,
        levels=dict(levels),
    )


def parse_toolkits(raw: Mapping[str, Any], source: str) -> dict[str, ToolkitSpec]:
    out: dict[str, ToolkitSpec] = {}
    for name, body in raw.items():
        where = f"{source}: [toolkit.{name}]"
        bad = [
            n
            for n in (name, *(body.get("servers", {}) if isinstance(body, Mapping) else {}))
            if not NAME_RE.match(str(n))
        ]
        if bad:
            raise ValueError(f"{where}: {bad[0]!r} is not a name (letters, digits, _ or -)")
        if not isinstance(body, Mapping):
            raise ValueError(f"{where} must be a table")
        unknown = sorted(set(body) - {"description", "skills", "servers"})
        if unknown:
            raise ValueError(f"{where} has unknown key(s): {', '.join(unknown)}")
        servers = body.get("servers", {})
        if not isinstance(servers, Mapping):
            raise ValueError(f"{where}.servers must be a table of servers")
        out[name] = ToolkitSpec(
            name=name,
            description=str(body.get("description", "")),
            grants={s: _parse_grant(name, s, g, source) for s, g in servers.items()},
            skills=_str_list(body.get("skills", []), f"{where}.skills"),
        )
    return out


def _q(s: str) -> str:
    """A TOML basic string: JSON's escaping is valid TOML, so nothing breaks out."""
    return json.dumps(str(s))


def _toml_list(items: tuple[str, ...]) -> str:
    return "[" + ", ".join(_q(i) for i in items) + "]"


def _toml_key(k: str) -> str:
    return k if re.fullmatch(r"[A-Za-z0-9_-]+", k) else _q(k)


def render_toolkits(toolkits: Mapping[str, ToolkitSpec]) -> list[str]:
    lines: list[str] = []
    for name, t in toolkits.items():
        lines.append(f"[toolkit.{_toml_key(name)}]")
        if t.description:
            lines.append(f"description = {_q(t.description)}")
        if t.skills:
            lines.append(f"skills = {_toml_list(t.skills)}")
        lines.append("")
        for server, g in t.grants.items():
            lines.append(f"[toolkit.{_toml_key(name)}.servers.{_toml_key(server)}]")
            if g.reads is not None:
                lines.append(f"reads = {_toml_list(g.reads)}")
            if g.writes is not None:
                lines.append(f"writes = {_toml_list(g.writes)}")
            if g.items is not None:
                lines.append(f"items = {_toml_list(g.items)}")
            if g.levels:
                lines.append(
                    "levels = { "
                    + ", ".join(f"{_toml_key(k)} = {_q(v)}" for k, v in g.levels.items())
                    + " }"
                )
            lines.append("")
    return lines


def merge(grants: list[Grant]) -> Grant:
    """Several toolkits granting one server: the union of what they allow (None, meaning
    everything, wins), and per operation the most cautious level any of them sets."""

    def union(vals: list[tuple[str, ...] | None]) -> tuple[str, ...] | None:
        if any(v is None for v in vals):
            return None
        return tuple(dict.fromkeys(x for v in vals for x in v or ()))

    levels: dict[str, str] = {}
    for g in grants:
        for op, lv in g.levels.items():
            levels[op] = tighter(levels[op], lv) if op in levels else lv
    return Grant(
        server=grants[0].server,
        reads=union([g.reads for g in grants]),
        writes=union([g.writes for g in grants]),
        items=union([g.items for g in grants]),
        levels=levels,
    )
