"""The write gate (ADR 0007, ADR 0011).

Every write from every skill comes through here. The operator's policy decides
how autonomous each operation is; the gate does the same work at every level:
allowlist check, snapshot, bounded steps, idempotency, stale check, readback,
audit row. At ``confirm`` a spoken yes sits between proposal and execution.
At ``announce`` and ``quiet`` the write happens at once and can be undone.
"""

from __future__ import annotations

from collections.abc import Callable
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Literal

from ghost_domain.confirmation import (
    ConfirmationClassifier,
    ConfirmationContext,
    RegexClassifier,
    Verdict,
)
from ghost_domain.models import (
    UNDO_WINDOW,
    AddComment,
    ChangeSet,
    DeleteComment,
    Outcome,
    Proposal,
    ProposalStatus,
    SetLabels,
    SetState,
    Snapshot,
    Target,
    utcnow,
)
from ghost_domain.policy import Autonomy, AutonomyPolicy, PolicyError, RepoPolicy, Risk
from ghost_domain.store import Store
from ghost_domain.writer import IssueWriter


class GateError(Exception):
    """A rule of the gate was violated. Safe to read aloud."""


@dataclass(frozen=True)
class Submission:
    """What ``submit`` hands back: the level that applied, the stored proposal, and
    the verified outcome when the write already happened."""

    autonomy: Autonomy
    proposal: Proposal
    outcome: Outcome | None

    @property
    def executed(self) -> bool:
        return self.outcome is not None


class WriteGate:
    """Holds at most one pending proposal per session.

    * ``submit`` is callable from an LLM tool. Depending on policy it either writes
      now or stores a pending proposal.
    * ``on_user_turn`` is called by the pipeline with the operator's transcribed
      turn. It is the only path from a pending proposal to ``execute``.
    * ``undo`` reverses the last executed change set in the session.
    """

    def __init__(
        self,
        store: Store,
        writer: IssueWriter,
        policy: RepoPolicy,
        *,
        autonomy: AutonomyPolicy | None = None,
        classifier: ConfirmationClassifier | None = None,
        undo_window: timedelta = UNDO_WINDOW,
        on_decision: Callable[[str], None] | None = None,
    ) -> None:
        self._store = store
        self._writer = writer
        self._policy = policy
        self._autonomy = autonomy or AutonomyPolicy()
        self._classifier = classifier or RegexClassifier()
        self._undo_window = undo_window
        self._log = on_decision or (lambda _msg: None)

    @property
    def autonomy(self) -> AutonomyPolicy:
        return self._autonomy

    def level(self, skill: str, operation: str, target: Target | None, risk: Risk) -> Autonomy:
        return self._autonomy.resolve(
            skill, operation, target.full_name if target else "", risk=risk
        )

    # --- submit ---------------------------------------------------------

    async def submit(
        self,
        *,
        session_id: str,
        skill: str,
        operation: str,
        target: Target,
        changes: ChangeSet,
        rationale: str,
        risk: Risk = "medium",
    ) -> Submission:
        self._policy.check(target)
        level = self.level(skill, operation, target, risk)
        if level is Autonomy.DENY:
            raise PolicyError(f"{skill}.{operation} is not allowed on {target.full_name}")
        proposal = await self._propose(
            session_id=session_id,
            skill=skill,
            operation=operation,
            target=target,
            changes=changes,
            rationale=rationale,
            autonomy=level,
        )
        self._log(f"gate: {skill}.{operation} on {target} -> {level.value}")
        if level is Autonomy.CONFIRM:
            return Submission(level, proposal, None)
        return Submission(level, proposal, await self._execute(proposal))

    async def prepare(
        self,
        *,
        session_id: str,
        skill: str,
        operation: str,
        target: Target,
        changes: ChangeSet,
        rationale: str,
    ) -> Proposal:
        """The confirm path regardless of policy. Writes nothing."""
        self._policy.check(target)
        return await self._propose(
            session_id=session_id,
            skill=skill,
            operation=operation,
            target=target,
            changes=changes,
            rationale=rationale,
            autonomy=Autonomy.CONFIRM,
        )

    async def _propose(
        self,
        *,
        session_id: str,
        skill: str,
        operation: str,
        target: Target,
        changes: ChangeSet,
        rationale: str,
        autonomy: Autonomy,
        undo_of: str | None = None,
    ) -> Proposal:
        # One pending proposal at a time: a new one replaces the old.
        self._store.cancel_pending(session_id)
        snapshot = await self._writer.fetch(target)
        proposal = Proposal(
            session_id=session_id,
            skill=skill,
            operation=operation,
            target=target,
            changes=changes,
            rationale=rationale,
            snapshot=snapshot,
            preview=_preview(target, snapshot, changes),
            autonomy=autonomy.value,
            undo_of=undo_of,
        )
        self._store.insert_proposal(proposal)
        self._store.event(
            proposal.id,
            "prepared",
            {"operation": operation, "target": str(target), "autonomy": autonomy.value},
        )
        return proposal

    def pending(self, session_id: str, now: datetime | None = None) -> Proposal | None:
        p = self._store.pending_for_session(session_id)
        if p is None:
            return None
        if p.is_expired(now):
            self._store.set_status(p.id, ProposalStatus.EXPIRED)
            self._store.event(p.id, "expired")
            return None
        return p

    def cancel(self, session_id: str, reason: str = "cancelled") -> Proposal | None:
        p = self.pending(session_id)
        if p is None:
            return None
        self._store.set_status(p.id, ProposalStatus.CANCELLED)
        self._store.event(p.id, "cancelled", {"reason": reason})
        return p

    # --- confirm + execute ---------------------------------------------

    async def on_user_turn(
        self,
        session_id: str,
        transcript: str,
        now: datetime | None = None,
        question: str = "",
    ) -> tuple[Verdict, Outcome | None]:
        """Called with the pipeline's own transcript of the operator's turn.

        Returns the verdict and, when the verdict was affirmative, the verified outcome.
        Any turn other than a clear affirmative cancels the pending proposal: the
        operator moved on, and a confirmation must directly follow the question.
        """
        p = self.pending(session_id, now)
        if p is None:
            return Verdict.UNCLEAR, None
        ctx = ConfirmationContext(pending_action=p.preview, question=question)
        verdict = await self._classifier.classify(transcript, ctx)
        self._store.event(
            p.id,
            "user_turn",
            {
                "verdict": verdict.value,
                "text": transcript,
                "classifier": getattr(self._classifier, "name", "custom"),
            },
        )
        if verdict is not Verdict.AFFIRMATIVE:
            self._store.set_status(p.id, ProposalStatus.CANCELLED)
            self._store.event(p.id, "cancelled", {"reason": verdict.value})
            return verdict, None
        return verdict, await self._execute(p, now)

    # --- undo ----------------------------------------------------------

    def undoable(self, session_id: str, now: datetime | None = None) -> Proposal | None:
        now = now or utcnow()
        return self._store.last_executed(session_id, now - self._undo_window)

    async def undo(self, session_id: str, now: datetime | None = None) -> Outcome:
        """Reverse the last executed change set in this session. Raises GateError when
        there is nothing to undo or the change cannot be reversed."""
        now = now or utcnow()
        last = self.undoable(session_id, now)
        if last is None:
            raise GateError("There is nothing recent to undo.")
        inverse = last.changes.invert(last.snapshot, last.idempotency_key)
        if inverse is None:
            raise GateError(f"The last change on {last.target} cannot be reversed.")
        proposal = await self._propose(
            session_id=session_id,
            skill=last.skill,
            operation="undo",
            target=last.target,
            changes=inverse,
            rationale=f"undo {last.operation} ({last.id})",
            autonomy=Autonomy.ANNOUNCE,
            undo_of=last.id,
        )
        self._log(f"gate: undo {last.skill}.{last.operation} on {last.target}")
        outcome = await self._execute(proposal, now)
        if outcome.status == "success":
            self._store.set_status(last.id, ProposalStatus.UNDONE)
            self._store.event(last.id, "undone", {"by": proposal.id})
        return outcome

    # --- execution -----------------------------------------------------

    async def _execute(self, p: Proposal, now: datetime | None = None) -> Outcome:
        now = now or utcnow()
        # Stale check: the issue must not have changed since the snapshot.
        current = await self._writer.fetch(p.target)
        if current.updated_at != p.snapshot.updated_at:
            self._store.set_status(p.id, ProposalStatus.STALE)
            self._store.event(
                p.id, "stale", {"was": p.snapshot.updated_at, "now": current.updated_at}
            )
            return Outcome(
                proposal_id=p.id,
                target=p.target,
                status="failed",
                applied=(),
                error="The issue changed since I read it. Ask me to look again.",
                readback=current,
            )
        if not self._store.claim(p.id, now):
            raise GateError("That proposal is no longer pending.")
        self._store.event(p.id, "claimed")

        applied: list[str] = []
        failed_step: str | None = None
        error: str | None = None
        for i, step in enumerate(p.changes.steps):
            label = step.describe()
            try:
                await self._apply(p, step)
            except Exception as e:  # noqa: BLE001 - every failure is recorded, never hidden
                failed_step, error = label, str(e)
                self._store.event(p.id, "step_failed", {"index": i, "step": label, "error": error})
                break
            applied.append(label)
            self._store.event(p.id, "step_applied", {"index": i, "step": label})

        try:
            readback: Snapshot | None = await self._writer.fetch(p.target)
        except Exception as e:  # noqa: BLE001
            readback = None
            self._store.event(p.id, "readback_failed", {"error": str(e)})

        status: Literal["success", "partial", "failed"]
        if failed_step is None:
            status = "success"
        elif applied:
            status = "partial"
        else:
            status = "failed"
        self._store.finish_operation(p.id, status)
        self._store.event(p.id, "finished", {"status": status})
        return Outcome(
            proposal_id=p.id,
            target=p.target,
            status=status,
            applied=tuple(applied),
            failed_step=failed_step,
            error=error,
            readback=readback,
        )

    async def _apply(
        self, p: Proposal, step: SetLabels | AddComment | DeleteComment | SetState
    ) -> None:
        match step:
            case SetLabels(add=add, remove=remove):
                await self._writer.set_labels(p.target, add, remove)
            case AddComment(body=body):
                await self._writer.add_comment(p.target, body, marker=p.idempotency_key)
            case DeleteComment(marker=marker):
                await self._writer.delete_comment(p.target, marker)
            case SetState(state=state):
                await self._writer.set_state(p.target, state)


def _preview(target: Target, snap: Snapshot, changes: ChangeSet) -> str:
    """Short, speakable summary of what the change set does to the target."""
    return f'On {target}, "{snap.title}": {changes.describe()}.'
