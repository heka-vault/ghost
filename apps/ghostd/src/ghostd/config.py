from __future__ import annotations

import os
from pathlib import Path

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


def runtime_dir() -> Path:
    base = Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "ghost"
    base.mkdir(parents=True, exist_ok=True, mode=0o700)
    return base


class GhostdSettings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="GHOST_", extra="ignore")

    # Skills
    skills: list[str] = ["github_issues"]
    github_token: str = Field(default="", alias="GITHUB_TOKEN")
    repos: str = ""  # GHOST_REPOS: comma-separated owner/repo allowlist
    github_mock: bool = False  # GHOST_GITHUB_MOCK=1: in-memory issues, no PAT needed

    # State
    db_path: Path = Path.home() / ".local" / "share" / "ghost" / "ghost.db"

    # Wake
    wake_mode: str = "file"  # file | onnx
    wake_model_path: Path | None = None
    wake_threshold: float = 0.5
    wake_cooldown_secs: float = 2.0

    # Audio
    duck_apps: list[str] = ["spotify"]  # matched against sink-input application names
    duck_level: float = 0.25
    chime: bool = True

    # Coexist with nerd-dictation (F9): its toggle script writes this file while active.
    dictation_flag: Path = (
        Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "dictation-audio-duck.state"
    )
