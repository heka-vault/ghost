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


def test_the_detector_threshold_follows_the_check(monkeypatch):
    """With Parakeet's check the threshold only decides how often it runs; without it the
    threshold is the whole decision, so it sits higher unless the operator set one."""
    from ghostd.config import GhostdSettings

    for name in (
        "GHOST_WAKE_THRESHOLD",
        "GHOST_WAKE_VERIFY",
        "GHOST_WAKE_ADDRESS_THRESHOLD",
        "GHOST_WAKE_ADDRESS_VERIFY",
    ):
        monkeypatch.delenv(name, raising=False)
    assert GhostdSettings().detector_threshold == 0.3
    assert GhostdSettings(wake_verify=False).detector_threshold == 0.7
    assert GhostdSettings(wake_verify=False, wake_threshold=0.5).detector_threshold == 0.5
    assert GhostdSettings(wake_verify=False).address_threshold == 0.7
    assert GhostdSettings(wake_address_threshold=0.6).address_threshold == 0.6
    checked = GhostdSettings(wake_verify=False, wake_address_verify=True)
    assert (checked.detector_threshold, checked.address_threshold) == (0.7, 0.3)


async def test_a_settings_change_rebuilds_the_listener_instead_of_waking(tmp_path, monkeypatch):
    """A live wake setting re-arms the passive loop: the pending wait is cancelled and a
    new one starts. It once fell through to the cancelled task's result and killed
    ghostd (2026-10-21)."""
    import asyncio

    from ghost_voice import VoiceSettings
    from ghostd.config import GhostdSettings
    from ghostd.main import Daemon

    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    d = Daemon(GhostdSettings(data_dir=tmp_path), VoiceSettings.model_construct())
    sessions = []

    async def conversation(*args, **kwargs):
        sessions.append(args)

    monkeypatch.setattr(d, "_conversation", conversation)

    class Wake:
        waits = 0

        async def wait(self):
            self.waits += 1
            (d._rearm if self.waits == 1 else d._stop).set()  # a settings change, then stop
            await asyncio.Event().wait()

    class Events:
        async def emit(self, *a, **k): ...

    wake = Wake()
    await asyncio.wait_for(d._passive(wake, None, [], None, Events(), None), 5)
    assert wake.waits == 2 and sessions == []


async def test_a_call_opens_the_session_in_that_line(tmp_path, monkeypatch):
    """The desktop's Call button (2026-10-24): a manual wake names the line the session
    opens in; a spoken wake still lands in Ghost, and a settled or unknown line is refused."""
    import asyncio

    from ghost_core.conversations import GHOST
    from ghost_voice import VoiceSettings
    from ghostd.config import GhostdSettings
    from ghostd.main import Daemon

    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    d = Daemon(GhostdSettings(data_dir=tmp_path, chime=False), VoiceSettings.model_construct())
    opened: list[str] = []

    async def conversation(*args, **kwargs):
        opened.append(kwargs.get("start_in", GHOST))

    monkeypatch.setattr(d, "_conversation", conversation)

    class Lines:
        def get(self, slug):
            class P:
                settled = slug == "old"

            return P() if slug in {"legal", "old"} else None

    d.conversations = Lines()  # type: ignore[assignment]

    class Manual:
        def trigger(self, text=None):
            manual.set()

    manual = asyncio.Event()
    d._manual = Manual()  # noqa: SLF001

    passive_again = asyncio.Event()  # the loop is waiting for its second wake

    class Wake:
        waits = 0

        async def wait(self):
            self.waits += 1
            if self.waits == 1:
                return None  # spoken: Ghost
            if self.waits == 2:
                passive_again.set()
                await manual.wait()
                return None
            d._stop.set()  # noqa: SLF001
            await asyncio.Event().wait()

    class Events:
        async def emit(self, *a, **k): ...

    wake = Wake()

    async def call_later():
        await passive_again.wait()
        assert await d.wake(None, "old") is False  # settled
        assert await d.wake(None, "nope") is False  # unknown
        assert await d.wake(None, "legal") is True

    task = asyncio.ensure_future(call_later())
    await asyncio.wait_for(d._passive(wake, None, [], None, Events(), None), 5)  # noqa: SLF001
    await task
    assert opened == [GHOST, "legal"]


async def test_an_early_chime_without_a_session_restores_the_music(tmp_path, monkeypatch):
    """The trained detector chimes and ducks before the operator finishes (ADR 0025). If
    something else wins first (a settings change here), the music comes back up, and the
    next session chimes as usual."""
    import asyncio

    from ghost_voice import VoiceSettings
    from ghostd.config import GhostdSettings
    from ghostd.main import Daemon

    monkeypatch.setenv("XDG_RUNTIME_DIR", str(tmp_path))
    d = Daemon(GhostdSettings(data_dir=tmp_path, chime=False), VoiceSettings.model_construct())

    class Ducker:
        calls: list[str] = []

        def duck(self):
            self.calls.append("duck")

        def restore(self):
            self.calls.append("restore")

    ducker = Ducker()
    d._ducker = ducker  # noqa: SLF001

    class Wake:
        waits = 0

        async def wait(self):
            self.waits += 1
            if self.waits == 1:
                await d._heard_early()  # noqa: SLF001 - the detector decided
                await d._heard_early()  # noqa: SLF001 - once is enough
                d._rearm.set()  # noqa: SLF001
            else:
                d._stop.set()  # noqa: SLF001
            await asyncio.Event().wait()

    class Events:
        async def emit(self, *a, **k): ...

    await asyncio.wait_for(d._passive(Wake(), None, [], None, Events(), ducker), 5)  # noqa: SLF001
    assert ducker.calls == ["duck", "restore"]
    assert d._chimed is False  # noqa: SLF001
