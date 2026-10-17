"""Ghost domain: the only code that may execute a write (ADR 0007).

Flow: a skill ``prepare``s a proposal -> the operator speaks a clear affirmative
(observed by the pipeline's own transcript, never LLM output) -> the gate claims
the proposal atomically, applies bounded steps, reads the target back, and
records the outcome in the audit log.
"""

from ghost_domain.confirmation import Verdict, classify_confirmation
from ghost_domain.gate import GateError, WriteGate
from ghost_domain.models import (
    AddComment,
    ChangeSet,
    Outcome,
    Proposal,
    ProposalStatus,
    SetLabels,
    SetState,
    Snapshot,
    Step,
    Target,
)
from ghost_domain.policy import RepoPolicy
from ghost_domain.store import Store
from ghost_domain.writer import IssueWriter

__all__ = [
    "AddComment",
    "ChangeSet",
    "GateError",
    "IssueWriter",
    "Outcome",
    "Proposal",
    "ProposalStatus",
    "RepoPolicy",
    "SetLabels",
    "SetState",
    "Snapshot",
    "Step",
    "Store",
    "Target",
    "Verdict",
    "WriteGate",
    "classify_confirmation",
]
