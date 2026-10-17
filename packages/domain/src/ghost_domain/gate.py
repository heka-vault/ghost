"""The write gate (ADR 0007)."""

from __future__ import annotations

from datetime import datetime
from typing import Literal

from ghost_domain.confirmation import Verdict, classify_confirmation
from ghost_domain.models import (
    AddComment,
    ChangeSet,
    Outcome,
    Proposal,
    ProposalStatus,
    SetLabels,
    SetState,
    Snapshot,
    Target,
    utcnow,
)
from ghost_domain.policy import RepoPolicy
from ghost_domain.store import Store
from ghost_domain.writer import IssueWriter


class GateError(Exception):
    """A rule of the gate was violated. Safe to read aloud."""


class WriteGate:
    """Holds at most one pending proposal per session.

    * ``prepare`` is callable from an LLM tool. It writes nothing to GitHub.
    * ``on_user_turn`` is called by the pipeline with the operator's transcribed
      turn. It is the only path to ``execute``.
    """

    def __init__(self, store: Store, writer: IssueWriter, policy: RepoPolicy) -> None:
        self._store = store
        self._writer = writer
        self._policy = policy

    # --- prepare -------------------------------------------------------

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
        self._policy.check(target)
        # One pending proposal at a time: a new one replaces the old.
        self._store.cancel_pending(session_id)
        snapshot = await self._writer.fetch(target)
        preview = _preview(target, snapshot, changes)
        proposal = Proposal(
            session_id=session_id,
            skill=skill,
            operation=operation,
            target=target,
            changes=changes,
            rationale=rationale,
            snapshot=snapshot,
            preview=preview,
        )
        self._store.insert_proposal(proposal)
        self._store.event(proposal.id, "prepared", {"operation": operation, "target": str(target)})
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
        self, session_id: str, transcript: str, now: datetime | None = None
    ) -> tuple[Verdict, Outcome | None]:
        """Called with the pipeline's own transcript of the operator's turn.

        Returns the verdict and, when the verdict was affirmative, the verified outcome.
        Any turn other than a clear affirmative cancels the pending proposal: the
        operator moved on, and a confirmation must directly follow the readback.
        """
        p = self.pending(session_id, now)
        if p is None:
            return Verdict.UNCLEAR, None
        verdict = classify_confirmation(transcript)
        self._store.event(p.id, "user_turn", {"verdict": verdict.value, "text": transcript})
        if verdict is not Verdict.AFFIRMATIVE:
            self._store.set_status(p.id, ProposalStatus.CANCELLED)
            self._store.event(p.id, "cancelled", {"reason": verdict.value})
            return verdict, None
        return verdict, await self._execute(p, now)

    async def _execute(self, p: Proposal, now: datetime | None = None) -> Outcome:
        now = now or utcnow()
        # Stale check: the issue must not have changed since the readback.
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

    async def _apply(self, p: Proposal, step: SetLabels | AddComment | SetState) -> None:
        match step:
            case SetLabels(add=add, remove=remove):
                await self._writer.set_labels(p.target, add, remove)
            case AddComment(body=body):
                await self._writer.add_comment(p.target, body, marker=p.idempotency_key)
            case SetState(state=state):
                await self._writer.set_state(p.target, state)


def _preview(target: Target, snap: Snapshot, changes: ChangeSet) -> str:
    """Short, speakable summary the agent reads before asking for confirmation."""
    return f'On {target}, "{snap.title}": {changes.describe()}. Say confirm to apply.'
