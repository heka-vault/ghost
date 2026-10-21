"""Named conversations with their own persona, voice and memory (ADR 0014)."""

from ghost_core.conversations.files import ConversationFiles
from ghost_core.conversations.models import (
    GHOST,
    Compaction,
    ConversationSnapshot,
    ConversationTarget,
    Profile,
    Scope,
    TurnRecord,
    describe_scope,
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
    "Scope",
    "TurnRecord",
    "conversations_source",
    "describe_scope",
    "slugify",
]
