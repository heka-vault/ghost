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

    # Turn taking. VAD values default to Pipecat's; raise confidence/min_volume
    # if a noisy room still triggers turns.
    vad_confidence: float = 0.7
    vad_start_secs: float = 0.2
    vad_stop_secs: float = 0.2
    vad_min_volume: float = 0.6
    # Words the STT must hear before the operator can cut Ghost off mid-sentence.
    # Backstop against echo; 0 disables barge-in.
    barge_in_min_words: int = 2
    idle_timeout_secs: float = 45.0
    smart_turn_threads: int = 2

    # Compaction (ADR 0014): Pipecat's in-session summarizer, tokens-only trigger. Gentle
    # on purpose; within one voice session it rarely fires. 0 disables it.
    compact_trigger_tokens: int = 24000
    compact_keep_messages: int = 20
    compact_summary_tokens: int = 3000

    # A soft loop while the model and tools are working (ghost_voice.thinking).
    quick_intents: bool = True  # exact-match sleep/cancel/undo without a model round trip
    thinking_sound: bool = True
    thinking_volume: float = 0.12
    thinking_delay_secs: float = 0.5

    # Audio devices (PortAudio index or name substring; None = system default, i.e. PipeWire)
    input_device: int | str | None = None
    output_device: int | str | None = None

    cache_dir: Path = Path.home() / ".cache" / "ghost"

    # Log transcripts, VAD and turn decisions (ghost_voice.debug). Off by default:
    # with it off, nothing Ghost hears while passive is written anywhere.
    debug_audio: bool = False
    log_level: str = "INFO"  # loguru level for everything, incl. Pipecat internals
