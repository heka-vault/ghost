"""Append-only edits to local text files, with undo by truncation.

The snapshot records the file's length, so undoing an append is cutting the file
back to that length, provided nothing else wrote in between (the version check
catches that)."""

from __future__ import annotations

import os
from pathlib import Path
from typing import Literal

from pydantic import Field

from ghost_core.models import Change, Snapshot, Target


class FileTarget(Target, frozen=True):
    path: Path

    def key(self) -> str:
        return f"file:{self.path}"

    def scope(self) -> str:
        return str(self.path.parent)

    def spoken(self) -> str:
        return self.path.name


class FileSnapshot(Snapshot, frozen=True):
    exists: bool
    length: int
    tail: str = ""  # last line, for the spoken form

    def spoken(self) -> str:
        if not self.exists:
            return "does not exist"
        return f"{self.length} characters, ending {self.tail!r}" if self.tail else "empty"


class AppendText(Change, frozen=True):
    kind: Literal["append"] = "append"
    text: str = Field(min_length=1, max_length=4000)

    def describe(self) -> str:
        return f"append: {self.text.strip()}"

    def invert(self, before: Snapshot, marker: str) -> Change | None:
        assert isinstance(before, FileSnapshot)
        return TruncateTo(length=before.length)


class TruncateTo(Change, frozen=True):
    """Only ever produced by undo."""

    kind: Literal["truncate"] = "truncate"
    length: int = Field(ge=0)

    def describe(self) -> str:
        return "remove what I appended"


class FileWriter:
    """Writes under ``root`` only. ``allowed`` is the matching check for Operation."""

    def __init__(self, root: Path) -> None:
        self.root = root.resolve()

    def allowed(self, target: Target) -> bool:
        return isinstance(target, FileTarget) and target.path.resolve().is_relative_to(self.root)

    async def fetch(self, target: Target) -> FileSnapshot:
        assert isinstance(target, FileTarget)
        p = target.path
        if not p.exists():
            return FileSnapshot(version="missing", exists=False, length=0)
        st = p.stat()
        text = p.read_text()
        tail = text.rstrip("\n").rsplit("\n", 1)[-1][-80:] if text.strip() else ""
        return FileSnapshot(
            version=f"{st.st_mtime_ns}:{st.st_size}", exists=True, length=len(text), tail=tail
        )

    async def apply(self, target: Target, change: Change, idempotency_key: str) -> None:
        assert isinstance(target, FileTarget)
        if not self.allowed(target):
            raise PermissionError(f"{target.path} is outside {self.root}")
        p = target.path
        match change:
            case AppendText(text=text):
                p.parent.mkdir(parents=True, exist_ok=True)
                with p.open("a") as f:
                    f.write(text if text.endswith("\n") else text + "\n")
            case TruncateTo(length=length):
                with p.open("r+") as f:
                    f.seek(0, os.SEEK_END)
                    if f.tell() > length:
                        f.truncate(length)
            case _:
                raise TypeError(f"FileWriter cannot apply {type(change).__name__}")
