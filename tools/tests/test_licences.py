"""Settings › About's licences (tools/licences.py) keep up with Cargo.lock."""

from __future__ import annotations

import json
import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def lock_names() -> set[str]:
    text = (ROOT / "Cargo.lock").read_text(encoding="utf-8")
    names = set()
    for block in text.split("[[package]]")[1:]:
        name = re.search(r'^name = "([^"]+)"', block, re.M)
        source = re.search(r"^source = ", block, re.M)
        if name and source:
            names.add(name.group(1))
    return names


def test_every_crate_from_a_registry_is_listed_with_a_licence():
    doc = json.loads((ROOT / "apps/desktop/public/licences.json").read_text(encoding="utf-8"))
    listed = {c["name"] for c in doc["rust"]}
    missing = sorted(lock_names() - listed)
    assert not missing, f"run `make licences`: {missing[:10]}"
    assert all(c["license"] for c in doc["rust"])
    assert doc["javascript"], "the desktop's own packages"
    assert doc["ghost"] == "MIT"
