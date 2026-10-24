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
    # (ADR 0010); onnx: the trained "hey ghost" detector, that phrase only, with no
    # transcript fallback (ADR 0025, ghost_voice.wakeword); file: `ghostd wake` /
    # SIGUSR1 only. `ghostd wake` works in every mode.
    wake_mode: str = "transcript"  # transcript | onnx | file
    wake_phrases: str = ""  # comma-separated; empty = ghost_voice.wake.DEFAULT_WAKE_PHRASES
    # Phonetic near-miss threshold for the wake phrase; 0 = exact only. 0.8 also took "he
    # set off", "her guests" and "hey guys" (2.1 false wakes an hour of read speech); 0.9
    # had none and kept the "he ghost" spellings (docs/benchmarks/2026-10-21-wake.md).
    wake_fuzzy: float = 0.9
    # onnx mode. The model; None: hey_ghost.onnx under the voice cache's wake/ folder.
    wake_model_path: Path | None = None
    # The detector's score (0-1) that counts as hearing "hey ghost". Unset: 0.3 with the
    # check on, where it only decides how often Parakeet runs (0.1 to 0.3 woke on the
    # same takes); 0.7 with it off, where it is the whole decision: every "hey ghost" at
    # the desk and no false wakes in 6 h of read speech, where 0.5 had two
    # (docs/benchmarks/2026-10-22-wake-detector.md).
    wake_threshold: float | None = None
    # Parakeet confirms each hit (match_wake at wake_fuzzy) before Ghost wakes; false:
    # the detector alone decides, and the chime plays the moment it fires.
    wake_verify: bool = True
    wake_cooldown_secs: float = 2.0

    # Models for addressing Ghost by name with a request ("Ghost, let's work on …"): this
    # file, or None for every ghost_*.onnx beside hey_ghost.onnx (one per opener).
    wake_address_model_path: Path | None = None
    # Parakeet checks their hits; unset: as wake_verify. The check lets them fire at a
    # low score: on the replay the per-opener models heard far more at 0.3 with it than
    # at 0.7 without it, and it turned down every false hit, so "hey ghost" can wake at
    # once while "Ghost, …" is checked (docs/benchmarks/2026-10-23-ghost-openers.md).
    wake_address_verify: bool | None = None
    # Their score that counts as a hit. Unset: 0.3 when they are checked, 0.7 when not.
    wake_address_threshold: float | None = None

    @property
    def detector_threshold(self) -> float:
        if self.wake_threshold is not None:
            return self.wake_threshold
        return 0.3 if self.wake_verify else 0.7

    @property
    def address_verify(self) -> bool:
        return self.wake_verify if self.wake_address_verify is None else self.wake_address_verify

    @property
    def address_threshold(self) -> float:
        if self.wake_address_threshold is not None:
            return self.wake_address_threshold
        return 0.3 if self.address_verify else 0.7

    # Audio
    duck_apps: list[str] = ["spotify"]  # matched against sink-input application names
    duck_level: float = 0.25
    chime: bool = True

    # Coexist with nerd-dictation (F9): its toggle script writes this file while active.
    dictation_flag: Path = (
        Path(os.environ.get("XDG_RUNTIME_DIR", "/tmp")) / "dictation-audio-duck.state"
    )
