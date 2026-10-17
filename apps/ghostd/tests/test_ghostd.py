import json

from ghostd import audio
from ghostd.dictation import is_dictating


def test_dictation_flag(tmp_path):
    flag = tmp_path / "dictation-audio-duck.state"
    assert is_dictating(flag) is False
    flag.write_text("sink\tno\n")
    assert is_dictating(flag) is True


def test_restore_from_corrupt_state_removes_file(tmp_path, monkeypatch):
    class FakePulse:
        def __init__(self, *_): ...
        def __enter__(self):
            return self

        def __exit__(self, *a): ...
        def sink_input_list(self):
            return []

    monkeypatch.setattr(audio, "pulsectl", type("M", (), {"Pulse": FakePulse}))
    f = tmp_path / "duck.json"
    f.write_text("{not json")
    audio.restore_from_state(f)
    assert not f.exists()


def test_restore_sets_saved_volumes(tmp_path, monkeypatch):
    calls = []

    class SI:
        index, name, corked = 7, "Spotify", False
        proplist = {"application.name": "spotify"}

        class volume:
            values = [0.2, 0.2]

    class FakePulse:
        def __init__(self, *_): ...
        def __enter__(self):
            return self

        def __exit__(self, *a): ...
        def sink_input_list(self):
            return [SI()]

        def volume_set(self, si, vol):
            calls.append((si.index, vol))

    monkeypatch.setattr(
        audio, "pulsectl", type("M", (), {"Pulse": FakePulse, "PulseVolumeInfo": list})
    )
    f = tmp_path / "duck.json"
    f.write_text(json.dumps({"7": [0.8, 0.8]}))
    audio.restore_from_state(f)
    assert calls == [(7, [0.8, 0.8])] and not f.exists()
