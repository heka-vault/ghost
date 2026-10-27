"""The four levels a change can run at (ADR 0011). The operator's words for them are the
permission lists (ADR 0034): ``block``, ``ask``, ``allow`` (announce) and ``quiet``."""

from __future__ import annotations

import enum


class Autonomy(enum.StrEnum):
    """How a change is handled, least to most autonomous."""

    DENY = "deny"  # blocked: not offered to the model at all
    CONFIRM = "confirm"  # ask: propose, ask, wait for a spoken yes, then apply
    ANNOUNCE = "announce"  # allow: apply now, say briefly what happened
    QUIET = "quiet"  # apply now, mention only if asked or if it failed
