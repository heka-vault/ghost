"""Ghost core: the write gate, the permissions, the audit, the agents and the apps
(ADR 0007, 0011, 0014, 0034).

Flow: a source (a connected app's tool, an agent operation) turns the model's arguments
into a typed change set -> the gate resolves the operator's permissions for that call ->
either the change happens now (``announce``/``quiet``) or it waits for a spoken yes
observed in the pipeline's own transcript (``confirm``) -> the steps, the audit log.
"""

from ghost_core.confirmation import (
    ConfirmationClassifier,
    ConfirmationContext,
    RegexClassifier,
    Verdict,
    classify_confirmation,
    has_negation,
)
from ghost_core.events import Events
from ghost_core.gate import GateError, Submission, WriteGate
from ghost_core.models import (
    Change,
    ChangeSet,
    Outcome,
    Proposal,
    ProposalStatus,
    Snapshot,
    Target,
)
from ghost_core.policy import Autonomy, AutonomyPolicy, PolicyError, Risk
from ghost_core.request import asked_for, number_mentions
from ghost_core.source import (
    Event,
    EventListener,
    Handled,
    Operation,
    Prepared,
    ProcessorFactory,
    ReadTool,
    Slot,
    ToolSource,
    TurnInterceptor,
    UserTurn,
)
from ghost_core.store import Store
from ghost_core.writer import Writer

__all__ = [
    "Autonomy",
    "AutonomyPolicy",
    "Change",
    "ChangeSet",
    "ConfirmationClassifier",
    "ConfirmationContext",
    "Event",
    "EventListener",
    "Events",
    "GateError",
    "Handled",
    "Operation",
    "Outcome",
    "PolicyError",
    "Prepared",
    "ProcessorFactory",
    "Proposal",
    "ProposalStatus",
    "ReadTool",
    "RegexClassifier",
    "Risk",
    "Slot",
    "Snapshot",
    "Store",
    "Submission",
    "Target",
    "ToolSource",
    "TurnInterceptor",
    "UserTurn",
    "Verdict",
    "WriteGate",
    "Writer",
    "asked_for",
    "classify_confirmation",
    "has_negation",
    "number_mentions",
]
