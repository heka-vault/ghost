"""Per-stream music ducking and the chime.

Rules from the workstation handoff: lower only the matching sink-inputs (Spotify),
restore their exact previous volumes, and never touch the sink mute that F8/F9 own.
The saved volumes live in a state file so ``ghostd restore-volumes`` can undo a
crash from a systemd ExecStopPost.
"""

from __future__ import annotations

import json
import math
from pathlib import Path

import numpy as np
from loguru import logger

try:
    import pulsectl
except ImportError:  # pragma: no cover
    pulsectl = None


class Ducker:
    def __init__(self, apps: list[str], level: float, state_file: Path) -> None:
        self._apps = [a.lower() for a in apps]
        self._level = level
        self._state = state_file

    def _matches(self, si) -> bool:
        props = si.proplist or {}
        names = [
            str(props.get("application.name", "")),
            str(props.get("application.process.binary", "")),
            str(si.name or ""),
        ]
        return any(a in n.lower() for a in self._apps for n in names)

    def duck(self) -> None:
        if pulsectl is None:
            return
        saved: dict[str, list[float]] = {}
        with pulsectl.Pulse("ghostd") as p:
            for si in p.sink_input_list():
                if not self._matches(si) or si.corked:
                    continue
                saved[str(si.index)] = list(si.volume.values)
                target = [min(v, self._level) for v in si.volume.values]
                p.volume_set(si, pulsectl.PulseVolumeInfo(target))
                logger.info(f"ducked {si.name!r} {si.volume.values} -> {target}")
        if saved:
            self._state.write_text(json.dumps(saved))

    def restore(self) -> None:
        restore_from_state(self._state)


def restore_from_state(state_file: Path) -> None:
    if pulsectl is None or not state_file.exists():
        return
    try:
        saved: dict[str, list[float]] = json.loads(state_file.read_text())
    except json.JSONDecodeError:
        state_file.unlink(missing_ok=True)
        return
    with pulsectl.Pulse("ghostd") as p:
        by_index = {str(si.index): si for si in p.sink_input_list()}
        for idx, vols in saved.items():
            si = by_index.get(idx)
            if si is None:
                continue  # stream ended; nothing to restore
            p.volume_set(si, pulsectl.PulseVolumeInfo(vols))
            logger.info(f"restored {si.name!r} -> {vols}")
    state_file.unlink(missing_ok=True)


def chime(up: bool = True, device: int | str | None = None) -> None:
    """Two short sine notes. Rising on wake, falling on sleep."""
    import sounddevice as sd

    sr = 24000
    notes = (660, 880) if up else (880, 660)
    parts = []
    for f in notes:
        t = np.arange(int(sr * 0.09)) / sr
        env = np.minimum(1.0, np.minimum(t / 0.01, (t[-1] - t) / 0.03))
        parts.append(0.25 * env * np.sin(2 * math.pi * f * t))
    audio = np.concatenate(parts).astype(np.float32)
    try:
        sd.play(audio, sr, device=device, blocking=True)
    except Exception as e:  # noqa: BLE001
        logger.warning(f"chime failed: {e}")
