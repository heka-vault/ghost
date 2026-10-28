"""Read and write KEY=value files (``settings.env``, and the plaintext files a password
import reads before it removes them) without ever echoing a value."""

from __future__ import annotations

import os
import re
import tempfile
from pathlib import Path

_LINE = re.compile(r"^\s*(?:export\s+)?([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*$")


def read_env(path: Path) -> dict[str, str]:
    if not path.exists():
        return {}
    out: dict[str, str] = {}
    for line in path.read_text().splitlines():
        if not line.strip() or line.lstrip().startswith("#"):
            continue
        m = _LINE.match(line)
        if m:
            value = m.group(2)
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "\"'":
                value = value[1:-1]
            out[m.group(1)] = value
    return out


def write_env(path: Path, values: dict[str, str], *, mode: int = 0o600) -> None:
    """Atomic, mode-restricted, one KEY=value per line. Comments in the old file are
    kept at the top so operator notes survive a UI edit."""
    path.parent.mkdir(parents=True, exist_ok=True, mode=0o700)
    comments = []
    if path.exists():
        comments = [ln for ln in path.read_text().splitlines() if ln.lstrip().startswith("#")]
    body = "\n".join(comments + [f"{k}={_quote(v)}" for k, v in sorted(values.items())]) + "\n"
    fd, tmp = tempfile.mkstemp(dir=path.parent, prefix=f".{path.name}.")
    try:
        os.fchmod(fd, mode)
        with os.fdopen(fd, "w") as f:
            f.write(body)
        os.replace(tmp, path)
    except BaseException:
        os.unlink(tmp)
        raise


def update_env(path: Path, changes: dict[str, str | None], *, mode: int = 0o600) -> dict[str, str]:
    values = read_env(path)
    for k, v in changes.items():
        if not re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", k):
            raise ValueError(f"invalid setting name {k!r}")
        if v is None:
            values.pop(k, None)
        else:
            values[k] = v
    write_env(path, values, mode=mode)
    return values


def _quote(v: str) -> str:
    if v == "" or any(c in v for c in " #\"'\\$\n\t"):
        return '"' + v.replace("\\", "\\\\").replace('"', '\\"') + '"'
    return v
