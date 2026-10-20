"""Model and endpoint settings. Everything comes from the environment (CLAUDE.md)."""

from __future__ import annotations

import re
from dataclasses import dataclass, field
from pathlib import Path
from urllib.parse import urlparse

from pydantic import Field, field_validator
from pydantic_settings import BaseSettings, SettingsConfigDict

LLM_ENDPOINTS = ("proxy", "openrouter")
OPENROUTER_ANTHROPIC_URL = "https://openrouter.ai/api"  # OpenRouter's Anthropic Messages API
_CLAUDE_ID = re.compile(r"^(claude-[a-z]+)-(\d+)(?:-(\d{1,2}))?$")


def openrouter_model(model: str) -> str:
    """Ghost's model id as OpenRouter names it: ``claude-sonnet-5-5`` is
    ``anthropic/claude-sonnet-5.5``. An id that already has a vendor prefix is kept."""
    if "/" in model:
        return model
    m = _CLAUDE_ID.match(model)
    if not m:
        return f"anthropic/{model}"
    name, major, minor = m.groups()
    return f"anthropic/{name}-{major}" + (f".{minor}" if minor else "")


@dataclass(frozen=True)
class LLMTarget:
    """Where model calls go. ``api_key`` is never in the repr; describe() leaves it out."""

    endpoint: str
    base_url: str | None
    model: str  # as the endpoint names it
    api_key: str = field(repr=False, default="")

    def describe(self) -> str:
        if self.base_url:
            u = urlparse(self.base_url)
            where = f"{u.scheme}://{u.hostname}" + (f":{u.port}" if u.port else "")
        else:
            where = "the SDK default"
        return f"{self.model} via {self.endpoint} ({where})"


class VoiceSettings(BaseSettings):
    model_config = SettingsConfigDict(env_prefix="GHOST_", extra="ignore")

    # LLM. GHOST_LLM_ENDPOINT picks the path:
    #   proxy       any Anthropic-compatible endpoint: ANTHROPIC_BASE_URL with
    #               ANTHROPIC_AUTH_TOKEN, read from the standard variables so one env file
    #               serves every tool (the default, as before)
    #   openrouter  OpenRouter's Anthropic Messages API with OPENROUTER_API_KEY (the key
    #               Jev can use too); GHOST_LLM_MODEL maps to OpenRouter's id
    # Keys come from the environment only and are never in a repr or a log line.
    llm_endpoint: str = "proxy"
    anthropic_base_url: str | None = Field(default=None, alias="ANTHROPIC_BASE_URL")
    anthropic_auth_token: str = Field(default="", alias="ANTHROPIC_AUTH_TOKEN", repr=False)
    openrouter_api_key: str = Field(default="", alias="OPENROUTER_API_KEY", repr=False)
    llm_model: str = "claude-sonnet-5-5"
    llm_thinking: str = "between_tools"  # Sonnet 5.5's thinking-off setting
    llm_max_tokens: int = 400

    @field_validator("llm_endpoint")
    @classmethod
    def _known_endpoint(cls, v: str) -> str:
        v = v.strip().lower()
        if v not in LLM_ENDPOINTS:
            raise ValueError(f"GHOST_LLM_ENDPOINT must be one of {', '.join(LLM_ENDPOINTS)}")
        return v

    def llm_target(self) -> LLMTarget:
        if self.llm_endpoint == "openrouter":
            return LLMTarget(
                endpoint="openrouter",
                base_url=OPENROUTER_ANTHROPIC_URL,
                model=openrouter_model(self.llm_model),
                api_key=self.openrouter_api_key,
            )
        return LLMTarget(
            endpoint="proxy",
            base_url=self.anthropic_base_url,
            model=self.llm_model,
            api_key=self.anthropic_auth_token,
        )

    def llm_problems(self) -> list[str]:
        """What stops the model from being reachable, without naming any value."""
        if self.llm_endpoint == "openrouter":
            return (
                []
                if self.openrouter_api_key
                else ["GHOST_LLM_ENDPOINT=openrouter but OPENROUTER_API_KEY is not set"]
            )
        problems = []
        if not self.anthropic_base_url:
            problems.append("ANTHROPIC_BASE_URL is not set")
        if not self.anthropic_auth_token:
            problems.append("ANTHROPIC_AUTH_TOKEN is not set")
        return problems

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

    # Turn taking. VAD values default to Pipecat's; raise confidence/min_volume
    # if a noisy room still triggers turns.
    vad_confidence: float = 0.7
    vad_start_secs: float = 0.2
    vad_stop_secs: float = 0.2
    vad_min_volume: float = 0.6
    # Words the STT must hear before the operator can cut Ghost off mid-sentence.
    # Backstop against echo; 0 disables barge-in.
    barge_in_min_words: int = 2
    # A transcript that mostly repeats what Ghost just said, heard while it speaks or this
    # long after, is its own voice coming back (late over the phone) and is dropped.
    echo_window_secs: float = 3.0
    # How long Ghost waits after the turn model says the operator is done before it
    # answers; speaking again in that time keeps the turn open. 0 answers at once.
    turn_grace_secs: float = 0.8
    # Smart Turn's fallback: when it judges an utterance incomplete, the turn still closes
    # after this much silence. 3 s upstream; 1.6 s here because it fired on finished
    # sentences and goodbyes waited for nothing (docs/performance.md, 2026-10-20).
    turn_stop_secs: float = 1.6
    idle_timeout_secs: float = 45.0
    # A session on hold (ADR 0021) hangs up quietly after this long without being addressed.
    hold_timeout_secs: float = 1800.0
    smart_turn_threads: int = 2

    # Crossing streams between scoped threads (ADR 0019); both off by default.
    # cross_department_answers: a scoped thread may read outside its scope to answer a
    # question, never write there. department_routing: a request that belongs to another
    # thread patches the operator through to it; off, Ghost says which thread and offers.
    cross_department_answers: bool = False
    department_routing: bool = False

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
