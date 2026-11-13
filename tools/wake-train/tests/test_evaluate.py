"""The operating point of a retrained detector (ADR 0065): chosen under the daemon's rule,
and refused when no point keeps false wakes at the shipped detector's. Synthetic scores."""

import numpy as np
from wake_train import evaluate


def stream(peaks: list[tuple[int, float]], n: int = 400) -> np.ndarray:
    s = np.full(n, 0.01)
    for i, v in peaks:
        s[i] = v
    return s


def test_the_first_frame_at_the_floor_decides():
    s = stream([(100, 0.35), (101, 0.9)])
    assert evaluate.rule_counts([s], 0.6, 0.3) == (0, 1)  # checked, not woken
    assert evaluate.rule_counts([s], 0.6, 0.5) == (1, 0)  # the floor skips the 0.35


def test_a_point_that_wakes_more_than_the_shipped_detector_is_not_chosen():
    old = [stream([(50, 0.4)])]
    new = [stream([(50, 0.85), (200, 0.7)])]
    p = evaluate.operating_point(old, new, lambda t, f: (1, 0), (0.6, 0.3))
    assert p is not None
    thr, floor, at = p
    assert thr > 0.85 and floor > 0.7
    assert at["false_wakes"] == 0 and at["asked"] <= 1


def test_none_when_every_point_trades_false_wakes():
    old = [stream([])]
    new = [stream([(50, 0.999)])]
    assert evaluate.operating_point(old, new, lambda t, f: (1, 0), (0.6, 0.3)) is None


def test_hits_respect_the_cooldown():
    s = stream([(10, 0.9), (20, 0.9), (60, 0.9)])  # 20 is 0.8 s after 10
    assert [round(t, 2) for t, _ in evaluate.hits(s, 0.6, 0.3)] == [0.8, 4.8]
