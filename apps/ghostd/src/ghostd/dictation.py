"""Pause wake detection while nerd-dictation (F9) is active."""

from __future__ import annotations

import subprocess
from pathlib import Path


def is_dictating(flag: Path) -> bool:
    if flag.exists():
        return True
    try:
        return (
            subprocess.run(
                ["pgrep", "-f", "/nerd-dictation begin"], capture_output=True, check=False
            ).returncode
            == 0
        )
    except FileNotFoundError:
        return False
