"""Model and endpoint settings. Everything comes from the environment (CLAUDE.md)."""

from __future__ import annotations

from pathlib import Path

from pydantic import Field
from pydantic_settings import BaseSettings, SettingsConfigDict


class VoiceSettings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="GHOST_", extra="ignore")

    # LLM: any Anthropic-compatible endpoint. The token is read from the
    # standard ANTHROPIC_* variables so one env file serves every tool.
    anthropic_base_url: str | None = Field(default=None, alias="ANTHROPIC_BASE_URL")
    anthropic_auth_token: str = Field(alias="ANTHROPIC_AUTH_TOKEN")
    llm_model: str = "claude-sonnet-5-5"
    llm_thinking: str = "between_tools"  # measured: 0.77 s to first token
    llm_max_tokens: int = 400

    # STT
    stt_model: str = "nemo-parakeet-tdt-0.6b-v3"
    stt_quantization: str | None = "int8"
    stt_threads: int = 4

    # TTS
    tts_voice: str = "af_heart"
    tts_speed: float = 1.0
    tts_model_path: Path | None = None
    tts_voices_path: Path | None = None

    # Turn taking
    vad_stop_secs: float = 0.2
    idle_timeout_secs: float = 45.0
    smart_turn_threads: int = 2

    # Audio devices (PortAudio index or name substring; None = system default, i.e. PipeWire)
    input_device: int | str | None = None
    output_device: int | str | None = None

    cache_dir: Path = Path.home() / ".cache" / "ghost"
