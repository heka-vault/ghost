"""Model and endpoint settings. Everything comes from the environment (CLAUDE.md)."""

from __future__ import annotations

import os
from collections.abc import Mapping
from pathlib import Path

from ghost_core.connection import LEGACY_ENDPOINTS as LLM_ENDPOINTS
from ghost_core.connection import OPENROUTER_ANTHROPIC_URL, ModelConnection, openrouter_model
from ghost_core.connection import Target as LLMTarget
from pydantic import Field, field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

TTS_ENGINES = ("kokoro", "elevenlabs")
# Output cap per reply, tool calls included: four comments asked for together are about
# 600 tokens, and the scaffold's 400 cut them off (ghost_voice.llm redoes a cut reply).
LLM_MAX_TOKENS = 2000

__all__ = [
    "LLM_ENDPOINTS",
    "LLM_FIELDS",
    "LLM_MAX_TOKENS",
    "OPENROUTER_ANTHROPIC_URL",
    "LLMTarget",
    "ModelConnection",
    "VoiceSettings",
    "openrouter_model",
]

# The VoiceSettings fields that make up the model connection, and the variables they
# mirror (ghost_core.connection). ghostd refreshes them when the desktop saves.
LLM_FIELDS = {
    "llm_provider": "GHOST_LLM_PROVIDER",
    "llm_endpoint": "GHOST_LLM_ENDPOINT",
    "llm_model": "GHOST_LLM_MODEL",
    "llm_protocol": "GHOST_LLM_PROTOCOL",
    "llm_base_url": "GHOST_LLM_BASE_URL",
    "llm_password": "GHOST_LLM_PASSWORD",
    "llm_headers": "GHOST_LLM_HEADERS",
    "llm_timeout_secs": "GHOST_LLM_TIMEOUT_SECS",
    "anthropic_base_url": "ANTHROPIC_BASE_URL",
    "cliproxy_base_url": "CLIPROXY_BASE_URL",
}


class VoiceSettings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="GHOST_", extra="ignore")

    # The model connection (ADR 0037, ghost_core.connection): a provider, its model, and
    # the name of the Password that holds its key. Without GHOST_LLM_PROVIDER the older
    # GHOST_LLM_ENDPOINT still decides (proxy: ANTHROPIC_BASE_URL with ANTHROPIC_AUTH_TOKEN;
    # openrouter: OpenRouter with OPENROUTER_API_KEY). Keys are read by name from the
    # environment when a target is built; no key is a field here, so none is in a repr.
    llm_provider: str = ""
    llm_endpoint: str = ""
    llm_model: str = ""
    llm_protocol: str = ""
    llm_base_url: str = ""
    llm_password: str = ""
    llm_headers: str = ""
    llm_timeout_secs: str = ""
    anthropic_base_url: str = Field(default="", alias="ANTHROPIC_BASE_URL")
    cliproxy_base_url: str = Field(default="", alias="CLIPROXY_BASE_URL")
    llm_thinking: str = "between_tools"  # Sonnet 5.5's thinking-off setting (Anthropic only)
    llm_max_tokens: int = LLM_MAX_TOKENS

    @field_validator("llm_endpoint")
    @classmethod
    def _known_endpoint(cls, v: str) -> str:
        v = v.strip().lower()
        if v and v not in LLM_ENDPOINTS:
            raise ValueError(f"GHOST_LLM_ENDPOINT must be one of {', '.join(LLM_ENDPOINTS)}")
        return v

    def llm_connection(self) -> ModelConnection:
        return ModelConnection.from_env(
            {env: str(getattr(self, f) or "") for f, env in LLM_FIELDS.items()}
        )

    def llm_target(self, env: Mapping[str, str] | None = None) -> LLMTarget:
        """Where model calls go now, the key read by name from ``env`` (the process
        environment by default, where the operator's Passwords are)."""
        return self.llm_connection().target(os.environ if env is None else env)

    def llm_problems(self, env: Mapping[str, str] | None = None) -> list[str]:
        """What stops the model from being reachable, without naming any value."""
        return self.llm_connection().problems(os.environ if env is None else env)

    # STT
    stt_model: str = "nemo-parakeet-tdt-0.6b-v3"
    stt_quantization: str | None = "int8"
    stt_threads: int = 4

    # TTS
    tts_voice: str = "af_heart"
    tts_speed: float = 1.0
    tts_model_path: Path | None = None
    tts_voices_path: Path | None = None
    # Speak from the first clause (a comma, colon, semicolon or dash after a few words)
    # instead of waiting for the whole sentence: Kokoro renders a chunk before any of it
    # plays, about 0.23 s per second of speech. False: whole sentences.
    tts_clauses: bool = True
    tts_clause_min_words: int = 4
    # The voice engine (proposed ADR 0026). kokoro: local, the default. elevenlabs: Flash
    # v2.5 over ElevenLabs' websocket, with Kokoro loaded beside it to speak instead when
    # the key or voice is missing or the socket fails (ghost_voice.elevenlabs). Only the
    # reply text goes there, never microphone audio. The key is never in a repr or a log.
    tts_engine: str = "kokoro"
    elevenlabs_api_key: str = Field(default="", alias="ELEVENLABS_API_KEY", repr=False)
    elevenlabs_voice: str = ""  # voice id for every line
    elevenlabs_voices: str = ""  # per line slug, "accounting=<voice id>,..."; others use the above
    elevenlabs_model: str = "eleven_flash_v2_5"

    @field_validator("tts_engine")
    @classmethod
    def _known_engine(cls, v: str) -> str:
        v = v.strip().lower()
        if v not in TTS_ENGINES:
            raise ValueError(f"GHOST_TTS_ENGINE must be one of {', '.join(TTS_ENGINES)}")
        return v

    # Turn taking. VAD values default to Pipecat's; raise confidence/min_volume
    # if a noisy room still triggers turns.
    vad_confidence: float = 0.7
    vad_start_secs: float = 0.2
    vad_stop_secs: float = 0.2
    vad_min_volume: float = 0.6
    # The passive wake listener's end of speech, longer than a turn's. At 0.2 s "Hey
    # Ghost, any new issues" splits at the comma and Parakeet hears the bare "hey ghost"
    # as "he goes"; 0.5 s caught 4 more of 40 wakes on the mic array, for 0.35 s more
    # before the chime (docs/benchmarks/2026-10-21-wake.md).
    wake_vad_stop_secs: float = 0.5
    # Words the STT must hear before the operator can cut Ghost off mid-sentence.
    # Backstop against echo; 0 disables barge-in.
    barge_in_min_words: int = 2
    # While Ghost speaks, Parakeet re-reads the operator's unfinished speech this often, so
    # those words count before they pause (ghost_voice.parakeet). 0: only after the pause.
    # 0.3 s cut Ghost off 1.0 s after the operator began instead of 2.4 s, for about 3.5
    # cores while both talk (docs/benchmarks/scripts/barge_in_bench.py, 2026-10-22).
    barge_in_interim_secs: float = 0.3
    # A transcript that mostly repeats what Ghost just said, heard while it speaks or this
    # long after, is its own voice coming back (late over the phone) and is dropped.
    echo_window_secs: float = 3.0
    # How long Ghost waits after the turn model says the operator is done before it
    # answers; speaking again in that time keeps the turn open. 0 answers at once.
    turn_grace_secs: float = 0.8
    # Start the model at the turn model's verdict instead of after the grace, holding its
    # answer until the turn is confirmed and dropping it if the operator goes on. Costs a
    # discarded model call each time they do; no tool runs early.
    turn_speculate: bool = True
    # Smart Turn's fallback: when it judges an utterance incomplete, the turn still closes
    # after this much silence. 3 s upstream; 1.6 s here because it fired on finished
    # sentences and goodbyes waited for nothing (docs/performance.md, 2026-10-20).
    turn_stop_secs: float = 1.6
    idle_timeout_secs: float = 45.0
    # A session on hold (ADR 0021) hangs up quietly after this long without being addressed.
    hold_timeout_secs: float = 1800.0
    smart_turn_threads: int = 2

    # Compaction (ADR 0014): Pipecat's in-session summarizer, tokens-only trigger. Gentle
    # on purpose; within one voice session it rarely fires. 0 disables it.
    compact_trigger_tokens: int = 24000
    compact_keep_messages: int = 20
    compact_summary_tokens: int = 3000

    # A soft loop while the model and tools are working (ghost_voice.thinking).
    quick_intents: bool = True  # exact-match sleep/cancel without a model round trip
    thinking_sound: bool = True
    thinking_volume: float = 0.12
    thinking_delay_secs: float = 0.5

    # Audio devices (PortAudio index or name substring; None = system default, i.e. PipeWire)
    input_device: int | str | None = None
    output_device: int | str | None = None

    cache_dir: Path = Path.home() / ".cache" / "ghost"

    # Per-turn latency (ghost_voice.latency): one INFO line and one JSON record per turn,
    # timings only. The record file is chosen by the caller (ghostd: data_dir).
    latency: bool = True
    latency_max_mb: float = 5.0

    # Log transcripts, VAD and turn decisions (ghost_voice.debug). Off by default:
    # with it off, nothing Ghost hears while passive is written anywhere.
    debug_audio: bool = False
    log_level: str = "INFO"  # loguru level for everything, incl. Pipecat internals
