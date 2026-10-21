"""Serialize proposals whose target, snapshot and changes are ability-defined types.

Each object is stored as ``{"$type": "module:Class", "data": {...}}``. On load the
class is imported and checked against the expected base, so a row can only ever
become a ``Target``, ``Snapshot`` or ``Change``. The database is local and
operator-owned (ADR 0009); this is a schema, not a trust boundary.
"""

from __future__ import annotations

import importlib
import json
from typing import Any, Literal

from pydantic import BaseModel

from ghost_core.models import Change, ChangeSet, Proposal, Snapshot, Target


class RetiredChange(Change, frozen=True):
    """A stored step whose class no longer exists (the undo-only steps Ghost once had,
    such as truncating a file or deleting its own comment). Loaded so the audit log
    still reads; no writer applies it."""

    kind: Literal["retired"] = "retired"
    type: str
    data: dict[str, Any]

    def describe(self) -> str:
        return f"{self.type} (no longer supported)"


class RetiredTarget(Target, frozen=True):
    """A stored target whose class is gone (an ability's, ADR 0022). The audit log keeps
    reading it by what the row holds; nothing can write to it again."""

    kind: Literal["retired"] = "retired"
    type: str
    data: dict[str, Any]

    def key(self) -> str:
        d = self.data
        if {"owner", "repo", "number"} <= set(d):
            return f"retired:{d['owner']}/{d['repo']}#{d['number']}"
        return f"retired:{self.type}:" + "/".join(str(v) for v in d.values())

    def scope(self) -> str:
        d = self.data
        return f"{d['owner']}/{d['repo']}" if {"owner", "repo"} <= set(d) else self.key()

    def spoken(self) -> str:
        return self.key().removeprefix("retired:")


class RetiredSnapshot(Snapshot, frozen=True):
    kind: Literal["retired"] = "retired"
    type: str
    data: dict[str, Any]

    def spoken(self) -> str:
        d = self.data
        parts = [str(d[k]) for k in ("state",) if k in d]
        if isinstance(d.get("labels"), list):
            parts.append("labels " + (", ".join(str(lb) for lb in d["labels"]) or "none"))
        return ", ".join(parts) or "as recorded"


def type_name(obj: BaseModel | type[BaseModel]) -> str:
    cls = obj if isinstance(obj, type) else type(obj)
    return f"{cls.__module__}:{cls.__qualname__}"


def dump(model: BaseModel) -> dict[str, Any]:
    return {"$type": type_name(model), "data": model.model_dump(mode="json")}


def load[M: BaseModel](d: dict[str, Any], base: type[M]) -> M:
    module, _, qual = str(d["$type"]).partition(":")
    obj: Any = importlib.import_module(module)
    for part in qual.split("."):
        obj = getattr(obj, part)
    if not (isinstance(obj, type) and issubclass(obj, base)):
        raise TypeError(f"{d['$type']} is not a {base.__name__}")
    return obj.model_validate(d["data"])


def dump_proposal(p: Proposal) -> str:
    body = p.model_dump(mode="json", exclude={"target", "snapshot", "changes"})
    body["target"] = dump(p.target)
    body["snapshot"] = dump(p.snapshot)
    body["changes"] = [dump(s) for s in p.changes.steps]
    return json.dumps(body)


def _load_change(d: dict[str, Any]) -> Change:
    try:
        return load(d, Change)
    except (ImportError, AttributeError):
        return RetiredChange(type=str(d["$type"]), data=d.get("data") or {})


def _load_target(d: dict[str, Any]) -> Target:
    try:
        return load(d, Target)
    except (ImportError, AttributeError):
        return RetiredTarget(type=str(d["$type"]), data=d.get("data") or {})


def _load_snapshot(d: dict[str, Any]) -> Snapshot:
    try:
        return load(d, Snapshot)
    except (ImportError, AttributeError):
        data = d.get("data") or {}
        return RetiredSnapshot(
            version=str(data.get("version", "")), type=str(d["$type"]), data=data
        )


def load_proposal(text: str) -> Proposal:
    body = json.loads(text)
    return Proposal(
        **{k: v for k, v in body.items() if k not in {"target", "snapshot", "changes"}},
        target=_load_target(body["target"]),
        snapshot=_load_snapshot(body["snapshot"]),
        changes=ChangeSet(steps=tuple(_load_change(s) for s in body["changes"])),
    )
