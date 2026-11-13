"""The retraining's data handling (ADR 0064, tools/wake-train/wake_train/data.py): what it
reads, what counts as the operator's voice, and what is held out. Tiny synthetic audio in a
temp folder; never the operator's."""

from __future__ import annotations

import json
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
sys.path.insert(0, str(ROOT / "tools" / "wake-train"))

from wake_train import data  # noqa: E402


def test_a_stream_line_reads_its_label_and_channel():
    s = data.parse_stream("/x/take-aec.wav:pos@12.3,40.1,~22")
    assert (s.kind, s.times, s.others, s.channel) == ("pos", (12.3, 40.1), (22.0,), 0)
    assert s.group == "take"
    assert data.parse_stream("/x/a.raw.wav#1:neg").channel == 1
    assert data.parse_stream("/x/a.wav:count=8").count == 8
    assert data.parse_stream("/x/a.wav:program=/p.json").program == "/p.json"


def test_a_take_and_its_array_stream_are_one_group():
    a = data.parse_stream("/x/over-ghost-195521-aec.wav:pos@1")
    r = data.parse_stream("/x/over-ghost-195521-raw.wav:pos@1")
    assert a.group == r.group == "over-ghost-195521"


def test_the_set_file_keeps_channels_and_skips_comments():
    text = """# a comment
[takes]
$S/a-aec.wav:pos@1.0
$S/b.raw.wav#0:neg

[more]
$H/*.aec.wav:neg
"""
    sets = data.read_sets(text, {"S": "/s", "H": "/h"})
    assert [s.title for s in sets] == ["takes", "more"]
    assert sets[0].streams[1].path == "/s/b.raw.wav"
    assert sets[1].streams[0].path == "/h/*.aec.wav"


def test_the_shipped_set_file_reads():
    text = (ROOT / "tools" / "wake-train" / "sets.txt").read_text()
    sets = data.read_sets(text, {"SCRATCH": "/s", "HEAR": "/h"})
    takes = [s for st in sets for s in st.streams if s.kind == "pos"]
    assert sum(len(s.times) for s in takes) == 42  # 21 takes, on ghost.mic and the array
    assert any(st.title.lower().startswith("kokoro") for st in sets)


def _kept(folder: Path, id_: str, outcome: str, label: str | None) -> None:
    data.write_wav(folder / f"{id_}.wav", [0, 100, -100] * 100)
    meta = {"id": id_, "outcome": outcome, "label": label, "score": 0.4, "input": "ghost.mic"}
    (folder / f"{id_}.json").write_text(json.dumps(meta))


def test_only_the_operators_labels_count_unless_they_said_so(tmp_path: Path):
    _kept(tmp_path, "a", "accepted", None)
    _kept(tmp_path, "b", "refused", None)
    _kept(tmp_path, "c", "refused", "me")
    _kept(tmp_path, "d", "accepted", "not")
    _kept(tmp_path, "e", "accepted", "me_missed_before")
    (tmp_path / "broken.json").write_text("{not json")
    data.write_wav(tmp_path / "broken.wav", [0])
    (tmp_path / "orphan.json").write_text(json.dumps({"id": "orphan", "outcome": "accepted"}))
    kept = {t.id: t for t in data.read_kept(tmp_path)}
    assert set(kept) == {"a", "b", "c", "d", "e"}
    labels = {k: data.label_of(t, False) for k, t in kept.items()}
    assert labels == {"a": None, "b": None, "c": "pos", "d": "neg", "e": "pos"}
    # an unlabelled wake Ghost accepted counts only when the operator says so; an
    # unlabelled refused hit never does
    assert data.label_of(kept["a"], True) == "pos"
    assert data.label_of(kept["b"], True) is None


def test_no_kept_takes_is_nothing(tmp_path: Path):
    assert data.read_kept(tmp_path / "missing") == []


def test_a_wav_reads_back(tmp_path: Path):
    p = tmp_path / "x.wav"
    data.write_wav(p, [1, -2, 3])
    assert data.read_wav(p) == [1, -2, 3]


def test_the_split_is_stable_and_never_empty():
    groups = [f"g{i}" for i in range(20)]
    train, held = data.split(groups, 0.3)
    assert train and held and not (train & held)
    assert train | held == set(groups)
    # a group held out today is held out tomorrow, with more groups
    train2, held2 = data.split([*groups, "new1", "new2"], 0.3)
    assert held <= held2
    assert data.split(["only"], 0.3) == ({"only"}, set()) or data.split(["only"], 0.3)[1] == {
        "only"
    }
    t, h = data.split(["a", "b"], 0.0)
    assert len(h) == 1 and len(t) == 1
    t, h = data.split(["a", "b"], 1.0)
    assert len(h) == 1 and len(t) == 1


def test_folds_keep_each_recording_whole_and_balance_takes():
    weight = {"big": 7, "mid": 6, "a": 3, "b": 2, "c": 1, "d": 1, "e": 1}
    fs = data.folds(weight, 3, weight)
    assert sorted(g for f in fs for g in f) == sorted(weight)
    loads = sorted(sum(weight[g] for g in f) for f in fs)
    assert loads[-1] - loads[0] <= 2
