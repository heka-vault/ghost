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

    data_dir: Path = Path.home() / ".local" / "share" / "ghost"

    # Autonomy (ADR 0011): the operator's per-operation levels. Missing file = every
    # operation falls back to its declared risk (labels/comments announce, close
    # confirms). Template: infra/policy.example.toml, seeded by `make env`.
    policy_path: Path = Path.home() / ".config" / "ghost" / "policy.toml"

    # Confirmation classifier. auto: Jev when TYPESAFE_API_KEY or OPENROUTER_API_KEY is
    # set, else the regex allowlist; on: Jev or fail at startup; off: regex only.
    jev: str = "auto"  # auto | on | off
    jev_threshold: float = 0.95
    jev_deadline_secs: float = 0.5
    typesafe_api_key: str = Field(default="", alias="TYPESAFE_API_KEY")
    openrouter_api_key: str = Field(default="", alias="OPENROUTER_API_KEY")

    # State
    db_path: Path = Path.home() / ".local" / "share" / "ghost" / "ghost.db"
    # Per-turn latency records (ghost_voice.latency), timings only; rotated at
    # GHOST_LATENCY_MAX_MB. Default: data_dir/latency.jsonl. `ghostd latency` summarizes.
    latency_log: Path | None = None

    @property
    def latency_path(self) -> Path:
        return self.latency_log or self.data_dir / "latency.jsonl"

    # Conversations (ADR 0014): persona.md, memory.md and notes.md per conversation live
    # here; the transcript and the registry are in the database. Summarized turns older
    # than the retention window are pruned daily.
    conversations_dir: Path = Path.home() / ".local" / "share" / "ghost" / "conversations"
    # Skills (ADR 0023): Ghost's own SKILL.md folders, plus project roots whose
    # .agents/skills are read in place: GHOST_SKILL_ROOTS=~/projects/x,~/projects/y
    skills_dir: Path = Path.home() / ".config" / "ghost" / "skills"
    skill_roots: str = ""
    # Department templates (ADR 0023): the operator's own; Ghost's shipped ones are in
    # infra/templates.
    templates_dir: Path = Path.home() / ".config" / "ghost" / "templates"

    @property
    def skill_root_paths(self) -> list[Path]:
        return [Path(p.strip()).expanduser() for p in self.skill_roots.split(",") if p.strip()]

    turn_retention_days: int = 90

    # Control API (ADR 0013): a user-owned Unix socket; no TCP. The desktop app, the
    # CLI and tests talk to it. Empty disables it.
    api_socket: Path | None = None  # default: $XDG_RUNTIME_DIR/ghost/api.sock
    settings_path: Path = Path.home() / ".config" / "ghost" / "settings.env"
    env_path: Path = Path.home() / ".config" / "ghost" / "env"
    vault_env_path: Path = Path.home() / ".config" / "heka-vault" / "ghost.env"
    # Where the PipeWire installer and unit templates live: the repo's infra/ in a
    # checkout, /usr/share/heka-ghost in the Debian package.
    infra_dir: Path = Path(__file__).resolve().parents[4] / "infra"

    # Phone listener (ADR 0016): plain HTTP with signed requests on one LAN address,
    # host:port (e.g. 192.168.1.20:8765). Empty = off. Not the control API. The paired
    # device and its secret are in data_dir/phone.json (0600). With ufw on, allow this
    # TCP port and UDP from the LAN (aiortc picks random UDP ports).
    phone_listen: str = ""

    # Wake. transcript: local VAD + Parakeet while passive, looking for a phrase
    # (ADR 0010); file: `ghostd wake` / SIGUSR1 only; onnx: trained model (pending).
    # `ghostd wake` works in every mode.
    wake_mode: str = "transcript"  # transcript | file | onnx
    wake_phrases: str = ""  # comma-separated; empty = ghost_voice.wake.DEFAULT_WAKE_PHRASES
    wake_fuzzy: float = 0.8  # phonetic near-miss threshold for the wake phrase; 0 = exact only
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
