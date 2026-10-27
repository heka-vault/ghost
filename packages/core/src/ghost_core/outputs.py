"""What Ghost makes (ADR 0033): a file in Ghost's ``out/`` folder, written through the gate.

Ghost does real work itself, code included: asked for a presentation, it writes the page.
``outputs.write`` creates a file and ``outputs.replace`` replaces one, each a gate operation
with its level, snapshot, read-back and audit row. The name is a plain file name with an
allowed extension, in ``out/`` and nowhere else, never through a link; the content is
capped. Ghost never runs what it makes: a page is opened by the operator, and a script runs
only from a skill, through ``ghost-scripts`` (ADR 0023).

There is one folder, ``~/.config/ghost/out/``, whichever agent made the file (ADR 0035).
"""

from __future__ import annotations

import hashlib
import os
import re
import tempfile
from pathlib import Path
from typing import Any, Literal

from pydantic import Field

from ghost_core.models import Change, ChangeSet, Snapshot, Target
from ghost_core.source import Operation, Prepared, ReadTool, ToolSource

SOURCE = "outputs"
OUT_DIR = "out"
MAX_BYTES = 256 * 1024
EXTENSIONS = (".html", ".md", ".txt", ".csv", ".json", ".svg", ".py", ".sh")
_NAME = re.compile(r"^[A-Za-z0-9][A-Za-z0-9 ._-]{0,100}$")


class OutputError(ValueError):
    pass


def check_name(name: str) -> str:
    """A plain file name: no directories, no dot files, an allowed extension."""
    n = name.strip()
    if not _NAME.fullmatch(n) or "/" in n or "\\" in n or ".." in n:
        raise OutputError(
            "an output is a plain file name, like week-in-review.html: no folders, no dots first"
        )
    if not n.lower().endswith(EXTENSIONS):
        raise OutputError(f"an output ends in one of {', '.join(EXTENSIONS)}")
    return n


def out_dir(config_dir: Path) -> Path:
    """Ghost's outputs: ``out/`` in its folder."""
    return Path(config_dir) / OUT_DIR


class OutputTarget(Target, frozen=True):
    name: str

    def key(self) -> str:
        return f"output:{self.name}"

    def scope(self) -> str:
        return self.name

    def spoken(self) -> str:
        return f"{self.name} in Ghost's outputs"

    def mentions(self) -> set[str]:
        stem = self.name.rsplit(".", 1)[0].lower()
        return {self.name.lower(), stem, *re.findall(r"[a-z0-9]{3,}", stem)}


class OutputSnapshot(Snapshot, frozen=True):
    exists: bool
    size: int = 0

    def spoken(self) -> str:
        return f"{self.size:,} bytes" if self.exists else "not there yet"


class WriteFile(Change, frozen=True):
    kind: Literal["write_file"] = "write_file"
    content: str = Field(max_length=MAX_BYTES)
    replace: bool = False

    def describe(self) -> str:
        size = len(self.content.encode())
        return f"{'replace it with' if self.replace else 'write'} {size:,} bytes"


class OutputWriter:
    def __init__(self, folder: Path) -> None:
        self._folder = folder

    def _path(self, target: Target) -> Path:
        assert isinstance(target, OutputTarget)
        folder = self._folder
        if folder.is_symlink():
            raise OutputError(f"{folder} is a link; Ghost writes only into a real out/ folder")
        path = folder / check_name(target.name)
        if path.is_symlink():
            raise OutputError(f"{path} is a link; Ghost does not write through links")
        if path.resolve().parent != folder.resolve() and folder.exists():
            raise OutputError("an output stays in out/")
        return path

    def allowed(self, target: Target) -> bool:
        if not isinstance(target, OutputTarget):
            return False
        try:
            self._path(target)
        except (OutputError, ValueError):
            return False
        return True

    async def fetch(self, target: Target) -> OutputSnapshot:
        p = self._path(target)
        if not p.is_file():
            return OutputSnapshot(version="missing", exists=False)
        data = p.read_bytes()
        return OutputSnapshot(
            version=hashlib.sha256(data).hexdigest()[:16], exists=True, size=len(data)
        )

    async def apply(self, target: Target, change: Change, idempotency_key: str) -> None:
        p = self._path(target)
        match change:
            case WriteFile(content=content, replace=replace):
                data = content.encode()
                if len(data) > MAX_BYTES:
                    raise OutputError(f"an output is at most {MAX_BYTES // 1024} KB")
                if p.exists() and not replace:
                    raise OutputError(f"{p.name} is already there; replacing it is its own yes")
                _write_atomic(p, data)
            case _:
                raise TypeError(f"OutputWriter cannot apply {type(change).__name__}")


def _write_atomic(p: Path, data: bytes) -> None:
    """Write beside the file, then rename over it: a reader never sees half a page."""
    p.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(dir=p.parent, prefix=".ghost-", suffix=".tmp")
    try:
        with os.fdopen(fd, "wb") as f:
            f.write(data)
        os.chmod(tmp, 0o644)  # readable; never executable: Ghost doesn't run it
        os.replace(tmp, p)
    except BaseException:
        Path(tmp).unlink(missing_ok=True)
        raise


def list_outputs(config_dir: Path) -> list[dict[str, Any]]:
    folder = out_dir(config_dir)
    if not folder.is_dir() or folder.is_symlink():
        return []
    out = []
    for p in sorted(folder.iterdir()):
        if p.is_symlink() or not p.is_file() or p.name.startswith("."):
            continue
        st = p.stat()
        out.append({"name": p.name, "path": str(p), "bytes": st.st_size, "modified": st.st_mtime})
    return out


class Outputs:
    """The ``outputs`` source: one read (what is in out/) and the two writes."""

    def __init__(self, config_dir: Path) -> None:
        self.config_dir = Path(config_dir)
        self.folder = out_dir(self.config_dir)
        self.writer = OutputWriter(self.folder)

    def _prepare(self, args: dict[str, Any], *, replace: bool) -> Prepared:
        name = check_name(str(args.get("name") or ""))
        content = str(args.get("content") or "")
        if not content.strip():
            raise OutputError("there is nothing to write")
        if len(content.encode()) > MAX_BYTES:
            raise OutputError(f"an output is at most {MAX_BYTES // 1024} KB; make it smaller")
        exists = (self.folder / name).is_file()
        if exists and not replace:
            raise OutputError(f"{name} is already there: use outputs.replace, or another name")
        if replace and not exists:
            raise OutputError(f"there is no {name} to replace: use outputs.write")
        return Prepared(
            target=OutputTarget(name=name),
            changes=ChangeSet(steps=(WriteFile(content=content, replace=replace),)),
            rationale=str(args.get("rationale") or f"{name}, made for the operator"),
        )

    def operations(self) -> list[Operation]:
        w = self.writer
        props = {
            "name": {
                "type": "string",
                "description": f"A plain file name ending in one of {', '.join(EXTENSIONS)}.",
            },
            "content": {"type": "string", "description": "The whole file."},
        }

        async def write(args: dict[str, Any]) -> Prepared:
            return self._prepare(args, replace=False)

        async def replace(args: dict[str, Any]) -> Prepared:
            return self._prepare(args, replace=True)

        return [
            Operation(
                "write",
                (
                    "Make a file for the operator (a page, a summary, a table, a small script) "
                    "in Ghost's outputs folder. Ghost never runs it. Say where it is "
                    "in a few words afterwards."
                ),
                props,
                ["name", "content"],
                prepare=write,
                writer=w,
                risk="low",
                allowed=w.allowed,
                asked_by=("make", "write", "create", "build", "draft", "put together"),
            ),
            Operation(
                "replace",
                "Replace a file already in Ghost's outputs with a new version.",
                props,
                ["name", "content"],
                prepare=replace,
                writer=w,
                risk="medium",
                allowed=w.allowed,
                asked_by=("replace", "update", "redo", "rewrite"),
            ),
        ]

    def read_tools(self) -> list[ReadTool]:
        async def listing(_args: dict[str, Any]) -> dict[str, Any]:
            return {
                "folder": "Ghost's outputs",
                "files": [
                    {"name": f["name"], "bytes": f["bytes"]} for f in list_outputs(self.config_dir)
                ],
            }

        return [
            ReadTool(
                "list_outputs",
                "The files Ghost has made in its outputs folder.",
                {},
                [],
                listing,
            )
        ]

    def source(self) -> ToolSource:
        return ToolSource(
            name=SOURCE,
            prompt=(
                "You can make self-contained things yourself (a page, a summary, a table, a "
                "small script) with outputs.write; they land in Ghost's outputs "
                "folder and you never run them. Sustained work inside a codebase belongs to "
                "the operator's coding tools, not you."
            ),
            read_tools=self.read_tools(),
            operations=self.operations(),
        )
