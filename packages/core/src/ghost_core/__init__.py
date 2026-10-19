"""Ghost core: the ability API, the write gate, policy and audit (ADR 0007, 0011, 0012).

Flow: an ability turns the model's arguments into a typed change set -> the gate
resolves the operator's autonomy policy for that operation -> either the write
happens now (``announce``/``quiet``) or it waits for a spoken yes observed in the
pipeline's own transcript (``confirm``) -> bounded steps, readback, audit log.
"""

from ghost_core.ability import (
    Ability,
    AbilityInstance,
    CliCommand,
    Contributions,
    Event,
    EventListener,
    Handled,
    Operation,
    Prepared,
    ProcessorFactory,
    ReadTool,
    SimpleInstance,
    Slot,
    TurnInterceptor,
    UserTurn,
)
from ghost_core.confirmation import (
    ConfirmationClassifier,
    ConfirmationContext,
    RegexClassifier,
    Verdict,
    classify_confirmation,
    has_negation,
)
from ghost_core.context import AbilityStore, GhostContext, MissingSecret, Secrets
from ghost_core.events import Events
from ghost_core.gate import GateError, Submission, WriteGate
from ghost_core.loader import AbilityError, LoadedAbility, discover, load
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
from ghost_core.scope import SCOPE_ARG, ActiveScope, OutOfScope, scope_of
from ghost_core.store import Store
from ghost_core.writer import Writer

__all__ = [
    "asked_for",
    "number_mentions",
    "scope_of",
    "SCOPE_ARG",
    "Ability",
    "ActiveScope",
    "OutOfScope",
    "AbilityError",
    "AbilityInstance",
    "AbilityStore",
    "Autonomy",
    "AutonomyPolicy",
    "Change",
    "ChangeSet",
    "CliCommand",
    "ConfirmationClassifier",
    "ConfirmationContext",
    "Contributions",
    "Event",
    "EventListener",
    "Events",
    "GateError",
    "GhostContext",
    "Handled",
    "LoadedAbility",
    "MissingSecret",
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
    "Secrets",
    "SimpleInstance",
    "Slot",
    "Snapshot",
    "Store",
    "Submission",
    "Target",
    "TurnInterceptor",
    "UserTurn",
    "Verdict",
    "WriteGate",
    "Writer",
    "classify_confirmation",
    "discover",
    "has_negation",
    "load",
]
