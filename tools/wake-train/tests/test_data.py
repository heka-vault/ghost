"""Reading the kept takes (ADR 0065): a take cut at the passive listener's start says so."""

import json
from pathlib import Path

from wake_train import data


def write_take(folder: Path, id_: str, **extra: object) -> None:
    data.write_wav(folder / f"{id_}.wav", [0] * 1600)
    meta = {"id": id_, "outcome": "accepted", "label": "me", "hit_at": 1.0, **extra}
    (folder / f"{id_}.json").write_text(json.dumps(meta))


def test_a_take_cut_at_the_listeners_start_is_read_as_cut(tmp_path: Path) -> None:
    write_take(tmp_path, "a", cut_at_start=True)
    write_take(tmp_path, "b")
    takes = {t.id: t for t in data.read_kept(tmp_path)}
    assert takes["a"].cut_at_start
    assert not takes["b"].cut_at_start
