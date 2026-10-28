"""What the operator blocks, or wants asked first (ADR 0034). Everything else runs.

A rule names an app, one of its tools, or a tool with argument patterns::

    gmail                                   every tool of the app
    github.delete_*                         a glob over the tool's name
    github.issue_write(state=closed)        only calls whose arguments match
    github.*(repo!=site)                    every call that is not about that repository
    conversations.delete                    one of Ghost's own operations

Each rule sits in one list, and the most careful list that matches wins: ``block``, then
``ask``, then ``quiet``, then ``allow``, then ``mode``, the default for a change no rule
names (``open`` announces it, ``ask`` asks first, ``read-only`` blocks it). A read is only
ever offered or blocked: ``ask`` and ``quiet`` say nothing about it. There is one set of
these, the operator's; nothing narrows or widens it per agent (ADR 0035).

A pattern is a shell glob, case-insensitive. ``arg=pattern`` holds when the argument is
there and matches (any item, for a list); ``arg!=pattern`` holds when it is missing or
does not match, so a ``block`` with ``!=`` also stops a call that leaves the argument out.
A rule with argument patterns can only be decided on a call: it never takes a tool away
from the model, it refuses the call.

The model never reads or writes any of this; nothing an app sends reaches it.
"""

from __future__ import annotations

import enum
import fnmatch
import re
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from ghost_core.levels import Autonomy

LISTS = ("block", "ask", "quiet", "allow")
_RULE = re.compile(
    r"""^\s*(?P<app>[A-Za-z0-9_*?\-/]+)
        (?:\.(?P<tool>[A-Za-z0-9_*?\-.]+))?
        \s*(?:\((?P<args>[^()]*)\))?\s*$""",
    re.X,
)
_COND = re.compile(r"^\s*(?P<arg>[A-Za-z0-9_\-.]+)\s*(?P<op>!=|=)\s*(?P<value>.*?)\s*$")


class Mode(enum.StrEnum):
    """What a change no rule names does."""

    OPEN = "open"  # it runs, and Ghost says so (announce)
    ASK = "ask"  # Ghost asks first (confirm)
    READ_ONLY = "read-only"  # it is blocked

    @property
    def level(self) -> Autonomy:
        return {
            Mode.OPEN: Autonomy.ANNOUNCE,
            Mode.ASK: Autonomy.CONFIRM,
            Mode.READ_ONLY: Autonomy.DENY,
        }[self]


@dataclass(frozen=True)
class Condition:
    arg: str
    value: str
    negated: bool = False

    def holds(self, args: Mapping[str, Any]) -> bool:
        raw = args.get(self.arg)
        values = raw if isinstance(raw, list | tuple) else ([] if raw is None else [raw])
        hit = any(_glob(str(v), self.value) for v in values)
        return not hit if self.negated else hit

    def __str__(self) -> str:
        return f"{self.arg}{'!=' if self.negated else '='}{self.value}"


@dataclass(frozen=True)
class Rule:
    """One line of a list. ``tool`` "*" is every tool of the app."""

    app: str
    tool: str = "*"
    conditions: tuple[Condition, ...] = ()

    @classmethod
    def parse(cls, text: str) -> Rule:
        m = _RULE.match(str(text))
        if m is None:
            raise ValueError(f"{text!r} is not a rule: write app, app.tool or app.tool(arg=value)")
        conditions: list[Condition] = []
        for part in (m["args"] or "").split(","):
            if not part.strip():
                continue
            c = _COND.match(part)
            if c is None or not c["value"]:
                raise ValueError(f"{text!r}: {part.strip()!r} is not arg=value or arg!=value")
            conditions.append(Condition(c["arg"], c["value"], negated=c["op"] == "!="))
        return cls(app=m["app"], tool=m["tool"] or "*", conditions=tuple(conditions))

    @property
    def conditional(self) -> bool:
        return bool(self.conditions)

    def names(self, source: str, operation: str) -> bool:
        """Whether the rule is about this tool at all, its argument patterns aside."""
        return _glob(source, self.app) and _glob(operation, self.tool)

    def matches(self, source: str, operation: str, args: Mapping[str, Any] | None) -> bool:
        """``args`` None is "no call yet": a rule with argument patterns cannot match."""
        if not self.names(source, operation):
            return False
        if not self.conditions:
            return True
        return args is not None and all(c.holds(args) for c in self.conditions)

    def __str__(self) -> str:
        text = self.app if self.tool == "*" else f"{self.app}.{self.tool}"
        if self.conditions:
            text += "(" + ", ".join(str(c) for c in self.conditions) + ")"
        return text


def _glob(value: str, pattern: str) -> bool:
    return fnmatch.fnmatchcase(value.lower(), pattern.lower())


def parse_rules(values: object, where: str) -> tuple[Rule, ...]:
    if not isinstance(values, list | tuple) or not all(isinstance(v, str) for v in values):
        raise ValueError(f"{where} must be a list of rules (strings)")
    out: list[Rule] = []
    for v in values:
        try:
            out.append(Rule.parse(v))
        except ValueError as e:
            raise ValueError(f"{where}: {e}") from None
    return tuple(out)


# Most careful first, the order of the lists (ADR 0034): block, ask, quiet, allow.
_CARE = (Autonomy.DENY, Autonomy.CONFIRM, Autonomy.QUIET, Autonomy.ANNOUNCE)


@dataclass(frozen=True)
class Permissions:
    """The operator's lists, and the mode for what they leave out."""

    mode: Mode = Mode.OPEN
    block: tuple[Rule, ...] = ()
    ask: tuple[Rule, ...] = ()
    quiet: tuple[Rule, ...] = ()
    allow: tuple[Rule, ...] = ()

    @classmethod
    def parse(cls, body: Mapping[str, Any], where: str) -> Permissions:
        """The ``[permissions]`` table."""
        unknown = sorted(set(body) - {*LISTS, "mode"})
        if unknown:
            raise ValueError(f"{where} has unknown key(s): {', '.join(unknown)}")
        mode = Mode.OPEN
        if "mode" in body:
            try:
                mode = Mode(str(body["mode"]))
            except ValueError:
                raise ValueError(
                    f"{where}.mode: {body['mode']!r} is not one of "
                    f"{', '.join(m.value for m in Mode)}"
                ) from None
        lists = {k: parse_rules(body.get(k, []), f"{where}.{k}") for k in LISTS}
        return cls(mode=mode, **lists)

    def with_rule(self, rule: Rule, level: str | None) -> Permissions:
        """The same lists with ``rule`` (by its text) moved to ``level``, or removed when
        ``level`` is None: what one toggle on the Apps screen does."""
        text = str(rule)
        lists = {k: tuple(r for r in getattr(self, k) if str(r) != text) for k in LISTS}
        if level is not None:
            if level not in LISTS:
                raise ValueError(f"{level!r} is not one of {', '.join(LISTS)}")
            lists[level] = (*lists[level], rule)
        return Permissions(mode=self.mode, **lists)

    def resolve(
        self, source: str, operation: str, args: Mapping[str, Any] | None = None
    ) -> Autonomy:
        """The level for a change: the most careful list that matches, else the mode."""
        return self._match(source, operation, args) or self.mode.level

    def _match(
        self, source: str, operation: str, args: Mapping[str, Any] | None
    ) -> Autonomy | None:
        lists = (self.block, self.ask, self.quiet, self.allow)
        for rules, level in zip(lists, _CARE, strict=True):
            if any(r.matches(source, operation, args) for r in rules):
                return level
        return None

    def blocks(self, source: str, operation: str, args: Mapping[str, Any] | None = None) -> bool:
        """Whether a read (or any call) is refused: only ``block`` can say so."""
        return any(r.matches(source, operation, args) for r in self.block)

    def offers_change(self, source: str, operation: str) -> bool:
        """Whether the model is offered the change at all: not when an unconditional
        ``block`` names it, nor in ``read-only`` when no rule could let a call through."""
        if self.blocks(source, operation):
            return False
        if self.mode is not Mode.READ_ONLY:
            return True
        return any(r.names(source, operation) for r in (*self.ask, *self.quiet, *self.allow))

    def offers_read(self, source: str, operation: str) -> bool:
        return not self.blocks(source, operation)

    def rules(self) -> dict[str, list[str]]:
        return {k: [str(r) for r in getattr(self, k)] for k in LISTS}

    def to_toml(self) -> list[str]:
        lines = ["[permissions]", f'mode = "{self.mode.value}"']
        for k in LISTS:
            rules = getattr(self, k)
            if rules:
                inner = ", ".join(_toml_str(str(r)) for r in rules)
                lines.append(f"{k} = [{inner}]")
        return lines


def _toml_str(s: str) -> str:
    return '"' + s.replace("\\", "\\\\").replace('"', '\\"') + '"'
