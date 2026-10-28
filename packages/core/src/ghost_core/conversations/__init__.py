"""Agents: named conversations with their own persona, job, voice and memory (ADR 0014,
ADR 0035)."""

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
    RECENT_TEMPLATE,
    SOURCE,
    SUMMARY_TEMPLATE,
    Conversations,
    ResumeContext,
    conversations_source,
)
from ghost_core.conversations.writer import ConversationWriter

__all__ = [
    "GHOST",
    "SOURCE",
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
    "conversations_source",
    "slugify",
]
