"""tools/loop_webp.py: the README loop's frame times.

Only the pure function is read, so the suite runs without the `shots` group's Pillow.
"""

from __future__ import annotations

from pathlib import Path

SOURCE = (Path(__file__).resolve().parent.parent / "loop_webp.py").read_text()


def durations(n: int, fps: float) -> list[int]:
    ns: dict[str, object] = {}
    exec(SOURCE[SOURCE.index("def durations") : SOURCE.index("def main")], ns)
    return ns["durations"](n, fps)  # type: ignore[operator]


def test_frame_times_add_up_to_the_loop_without_drift() -> None:
    d = durations(360, 30)
    assert sum(d) == 12_000
    assert set(d) <= {33, 34}
