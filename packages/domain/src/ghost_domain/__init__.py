"""Ghost domain: the only code that may execute a write (ADR 0007, ADR 0011).

Flow: a skill turns the model's arguments into a typed change set -> the gate
resolves the operator's autonomy policy for that operation -> either the write
happens now (``announce``/``quiet``) or it waits for a spoken yes observed in the
pipeline's own transcript (``confirm``) -> bounded steps, readback, audit log.
The last executed change can be undone.
"""

from ghost_domain.confirmation import (
    ConfirmationClassifier,
    ConfirmationContext,
    RegexClassifier,
    Verdict,
    classify_confirmation,
    has_negation,
)
from ghost_domain.gate import GateError, Submission, WriteGate
from ghost_domain.models import (
    AddComment,
    ChangeSet,
    DeleteComment,
    Outcome,
    Proposal,
    ProposalStatus,
    SetLabels,
    SetState,
    Snapshot,
    Step,
    Target,
)
from ghost_domain.policy import Autonomy, AutonomyPolicy, PolicyError, RepoPolicy, Risk
from ghost_domain.store import Store
from ghost_domain.writer import IssueWriter

__all__ = [
    "AddComment",
    "Autonomy",
    "AutonomyPolicy",
    "ChangeSet",
    "ConfirmationClassifier",
    "ConfirmationContext",
    "DeleteComment",
    "GateError",
    "IssueWriter",
    "Outcome",
    "PolicyError",
    "Proposal",
    "ProposalStatus",
    "RegexClassifier",
    "RepoPolicy",
    "Risk",
    "SetLabels",
    "SetState",
    "Snapshot",
    "Step",
    "Store",
    "Submission",
    "Target",
    "Verdict",
    "WriteGate",
    "classify_confirmation",
    "has_negation",
]
