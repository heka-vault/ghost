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


def load_proposal(text: str) -> Proposal:
    body = json.loads(text)
    return Proposal(
        **{k: v for k, v in body.items() if k not in {"target", "snapshot", "changes"}},
        target=load(body["target"], Target),
        snapshot=load(body["snapshot"], Snapshot),
        changes=ChangeSet(steps=tuple(_load_change(s) for s in body["changes"])),
    )
