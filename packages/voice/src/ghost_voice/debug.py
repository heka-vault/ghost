"""Audio debug logging, off by default.

With ``GHOST_DEBUG_AUDIO=1`` Ghost logs what it hears and decides: passive and
session transcripts, VAD starts and stops, wake matches and misses, barge-in
decisions, and confirmation verdicts. The log stays on the machine (journal or
terminal). Leave it off in normal use: with it off, passive transcripts are
never written anywhere (CLAUDE.md, ADR 0010).
"""

from __future__ import annotations

from loguru import logger

enabled = False


def enable(on: bool = True) -> None:
    global enabled
    enabled = on
    if on:
        logger.warning("GHOST_DEBUG_AUDIO is on: transcripts and audio decisions are being logged")


def audio(msg: str) -> None:
    """Log an audio-path event when debugging is on."""
    if enabled:
        logger.info(f"[audio] {msg}")
