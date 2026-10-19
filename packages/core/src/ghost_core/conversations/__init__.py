"""Named conversations with their own persona, voice and memory (ADR 0014)."""

from ghost_core.conversations.files import ConversationFiles
from ghost_core.conversations.models import (
    GHOST,
    Compaction,
    ConversationSnapshot,
    ConversationTarget,
    Profile,
    TurnRecord,
    slugify,
)
from ghost_core.conversations.service import (
    ABILITY,
    RECENT_TEMPLATE,
    SUMMARY_TEMPLATE,
    Conversations,
    ResumeContext,
)
from ghost_core.conversations.writer import ConversationWriter

__all__ = [
    "ABILITY",
    "GHOST",
    "RECENT_TEMPLATE",
    "SUMMARY_TEMPLATE",
    "Compaction",
    "ConversationFiles",
    "ConversationSnapshot",
    "ConversationTarget",
    "ConversationWriter",
    "Conversations",
    "Profile",
    "ResumeContext",
    "TurnRecord",
    "slugify",
]
