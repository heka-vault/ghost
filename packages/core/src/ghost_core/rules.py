"""Ghost's rules as a tool source (ADR 0035): one operation, ``rules.add``.

``AGENTS.md`` in Ghost's folder is in every agent's prompt. ``rules.add`` appends the
operator's words to it, verbatim, like a persona change (ADR 0014): Ghost never
paraphrases, reorders or rewrites that file. The operation has no ``asked_by``, so where
the permissions ask first a plain request is never the yes: the operator hears the exact
text read back and says yes (the words the model wrote are what gets written, so the
readback is the check). On the spoken path the text must also be in the operator's
request, word for word. Nothing else in Ghost's folder (``apps.json``,
``watches.json``, ``hooks.json``, ``policy.toml``) has a voice path at all.
"""

from __future__ import annotations

import hashlib
import re
from pathlib import Path
from typing import Any

from pydantic import Field

from ghost_core.folder import RULES_FILE
from ghost_core.models import Change, ChangeSet, Snapshot, Target
from ghost_core.source import Operation, Prepared, ToolSource

SOURCE = "rules"  # the gate, policy and audit key
MAX_RULE = 1000
HEADING = "# Ghost"  # the first line of a new AGENTS.md


class RulesTarget(Target, frozen=True):
    """Ghost's AGENTS.md."""

    def key(self) -> str:
        return "rules:ghost"

    def scope(self) -> str:
        return "ghost"

    def spoken(self) -> str:
        return "Ghost's rules"

    def mentions(self) -> set[str]:
        return {"rule", "rules"}


class RulesSnapshot(Snapshot, frozen=True):
    rules: str = ""

    def spoken(self) -> str:
        lines = self.rules.count("\n") + 1 if self.rules.strip() else 0
        return f"{lines} line(s) of rules"


class AddRule(Change, frozen=True):
    """The operator's words, appended to AGENTS.md as they were said."""

    kind: str = "add_rule"
    text: str = Field(min_length=1, max_length=MAX_RULE)

    def describe(self) -> str:
        return f"add the rule: {self.text.strip()}"


def _read(path: Path) -> str:
    try:
        return path.read_text() if path.is_file() and not path.is_symlink() else ""
    except OSError:
        return ""


def _append(path: Path, text: str) -> None:
    """Append ``text`` as its own paragraph, under a heading when the file is new."""
    if path.is_symlink():
        raise ValueError(f"{path} is a link; Ghost writes only regular files in its own folder")
    path.parent.mkdir(parents=True, exist_ok=True)
    current = _read(path)
    if not current.strip():
        current = f"{HEADING}\n"
    if not current.endswith("\n"):
        current += "\n"
    if not current.endswith("\n\n"):
        current += "\n"
    path.write_text(f"{current}{text.strip()}\n")


def said(text: str, request: str) -> bool:
    """Whether ``text`` is in the operator's request, word for word (case, punctuation
    and spacing aside). The spoken path passes the request; the desktop, where the
    operator typed the rule, does not."""

    def norm(s: str) -> str:
        return " ".join(re.findall(r"[a-z0-9']+", s.lower()))

    return bool(norm(text)) and norm(text) in norm(request)


class RulesWriter:
    def __init__(self, path: Path) -> None:
        self.path = path

    def allowed(self, target: Target) -> bool:
        return isinstance(target, RulesTarget)

    async def fetch(self, target: Target) -> RulesSnapshot:
        rules = _read(self.path)
        return RulesSnapshot(version=hashlib.sha256(rules.encode()).hexdigest()[:16], rules=rules)

    async def apply(self, target: Target, change: Change, idempotency_key: str) -> None:
        match change:
            case AddRule(text=text):
                _append(self.path, text)
            case _:
                raise TypeError(f"RulesWriter cannot apply {type(change).__name__}")


class Rules:
    """Ghost's ``AGENTS.md``: read for the prompt and the desktop, added to through the gate."""

    def __init__(self, config_dir: Path) -> None:
        self.path = Path(config_dir).expanduser() / RULES_FILE
        self.writer = RulesWriter(self.path)

    def text(self) -> str:
        return _read(self.path)

    def prepare_add(self, text: str, request: str | None = None) -> Prepared:
        """The change that appends ``text``. ``request`` is the operator's spoken request,
        when there is one: the rule must be in it, word for word."""
        text = text.strip()
        if not text:
            raise ValueError("the rule is required, in the operator's words")
        if len(text) > MAX_RULE:
            raise ValueError("that rule is too long; keep it to a few sentences")
        if request is not None and not said(text, request):
            raise ValueError(
                "a rule is written in the operator's words: pass exactly what they "
                "said after 'add a rule', nothing rephrased"
            )
        return Prepared(
            target=RulesTarget(),
            changes=ChangeSet(steps=(AddRule(text=text),)),
            rationale="a rule for every agent, in the operator's words",
        )

    def operations(self) -> list[Operation]:
        w = self.writer

        async def add(args: dict[str, Any]) -> Prepared:
            request = str(args.get("_request") or "") if "_request" in args else None
            return self.prepare_add(str(args.get("text") or ""), request)

        return [
            Operation(
                "add",
                (
                    "Add a standing rule for every agent to Ghost's AGENTS.md, in the "
                    "operator's own words, verbatim: 'add a rule: always use UK dates'. Never "
                    "paraphrase, never rewrite what is there. The operator hears the exact "
                    "text back and says yes before it is written."
                ),
                {"text": {"type": "string"}},
                ["text"],
                prepare=add,
                writer=w,
                risk="medium",
                allowed=w.allowed,
            ),
        ]

    def source(self) -> ToolSource:
        return ToolSource(name=SOURCE, prompt="", read_tools=[], operations=self.operations())


def rules_source(rules: Rules) -> ToolSource:
    return rules.source()


__all__ = ["SOURCE", "AddRule", "Rules", "RulesSnapshot", "RulesTarget", "rules_source", "said"]
