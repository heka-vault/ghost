"""The write gate (ADR 0007, ADR 0011, ADR 0012).

Every write from every ability comes through here. Abilities register their
operations at startup; the operator's policy decides how autonomous each one is; the
gate does the same work at every level: the ability's allowlist, snapshot, bounded
steps, idempotency, stale check, readback, audit row. At ``confirm`` a spoken yes
sits between proposal and execution. At ``announce`` and ``quiet`` the write happens
at once and can be undone.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable
from dataclasses import dataclass
from datetime import datetime, timedelta

from ghost_core.ability import Operation
from ghost_core.confirmation import (
    ConfirmationClassifier,
    ConfirmationContext,
    RegexClassifier,
    Verdict,
)
from ghost_core.models import (
    UNDO_WINDOW,
    ChangeSet,
    Outcome,
    Proposal,
    ProposalStatus,
    Snapshot,
    Target,
    utcnow,
)
from ghost_core.policy import Autonomy, AutonomyPolicy, PolicyError
from ghost_core.store import Store


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
        *,
        autonomy: AutonomyPolicy | None = None,
        classifier: ConfirmationClassifier | None = None,
        undo_window: timedelta = UNDO_WINDOW,
        on_decision: Callable[[str], None] | None = None,
    ) -> None:
        self._store = store
        self._autonomy = autonomy or AutonomyPolicy()
        self._classifier = classifier or RegexClassifier()
        self._undo_window = undo_window
        self._log = on_decision or (lambda _msg: None)
        self._ops: dict[tuple[str, str], Operation] = {}

    # --- registry ------------------------------------------------------

    def register(self, ability: str, operations: Iterable[Operation]) -> None:
        for op in operations:
            self._ops[(ability, op.name)] = op

    def operation(self, ability: str, name: str) -> Operation:
        try:
            return self._ops[(ability, name)]
        except KeyError:
            raise GateError(f"unknown operation {ability}.{name}") from None

    def operations(self, ability: str) -> list[Operation]:
        return [op for (a, _), op in self._ops.items() if a == ability]

    @property
    def autonomy(self) -> AutonomyPolicy:
        return self._autonomy

    def set_autonomy(self, policy: AutonomyPolicy) -> None:
        """Swap the operator's policy live (the file was edited through the UI)."""
        self._autonomy = policy
        self._log(f"gate: autonomy policy reloaded from {policy.source}")

    def level(self, ability: str, operation: str, target: Target | None = None) -> Autonomy:
        op = self.operation(ability, operation)
        return self._autonomy.resolve(
            ability, operation, target.scope() if target else "", risk=op.risk
        )

    # --- submit ---------------------------------------------------------

    async def submit(
        self,
        *,
        session_id: str,
        ability: str,
        operation: str,
        target: Target,
        changes: ChangeSet,
        rationale: str,
    ) -> Submission:
        op = self.operation(ability, operation)
        if not op.allowed(target):
            raise PolicyError(f"{target.spoken()} is not allowed for {ability}.{operation}")
        level = self.level(ability, operation, target)
        if level is Autonomy.DENY:
            raise PolicyError(f"{ability}.{operation} is not allowed on {target.scope()}")
        proposal = await self._propose(
            session_id=session_id,
            ability=ability,
            operation=operation,
            target=target,
            changes=changes,
            rationale=rationale,
            autonomy=level,
        )
        self._log(f"gate: {ability}.{operation} on {target} -> {level.value}")
        if level is Autonomy.CONFIRM:
            return Submission(level, proposal, None)
        return Submission(level, proposal, await self._execute(proposal))

    async def prepare(
        self,
        *,
        session_id: str,
        ability: str,
        operation: str,
        target: Target,
        changes: ChangeSet,
        rationale: str,
    ) -> Proposal:
        """The confirm path regardless of policy. Writes nothing."""
        op = self.operation(ability, operation)
        if not op.allowed(target):
            raise PolicyError(f"{target.spoken()} is not allowed for {ability}.{operation}")
        return await self._propose(
            session_id=session_id,
            ability=ability,
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
        ability: str,
        operation: str,
        target: Target,
        changes: ChangeSet,
        rationale: str,
        autonomy: Autonomy,
        undo_of: str | None = None,
    ) -> Proposal:
        writer = self.operation(ability, operation.removeprefix("undo:")).writer
        # One pending proposal at a time: a new one replaces the old.
        self._store.cancel_pending(session_id)
        snapshot = await writer.fetch(target)
        proposal = Proposal(
            session_id=session_id,
            ability=ability,
            operation=operation,
            target=target,
            changes=changes,
            rationale=rationale,
            snapshot=snapshot,
            preview=f"On {target.spoken()}: {changes.describe()}.",
            autonomy=autonomy.value,
            undo_of=undo_of,
        )
        self._store.insert_proposal(proposal)
        self._store.event(
            proposal.id,
            "prepared",
            {
                "ability": ability,
                "operation": operation,
                "target": target.key(),
                "autonomy": autonomy.value,
            },
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

    async def confirm_pending(
        self, session_id: str, *, source: str = "ui", now: datetime | None = None
    ) -> Outcome | None:
        """An authenticated local action (a click in the operator's own desktop session)
        confirms the pending proposal (ADR 0007 as amended by ADR 0013). Returns None
        when nothing is pending."""
        p = self.pending(session_id, now)
        if p is None:
            return None
        self._store.event(
            p.id, "user_turn", {"verdict": Verdict.AFFIRMATIVE.value, "classifier": source}
        )
        self._log(f"gate: {source} confirmed {p.id}")
        return await self._execute(p, now)

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
        # Undo is safe only if nobody else touched the target since our write finished.
        writer = self.operation(last.ability, last.writer_operation).writer
        current = await writer.fetch(last.target)
        was = self._store.readback_version(last.id)
        if was is not None and current.version != was:
            self._store.event(last.id, "undo_stale", {"was": was, "now": current.version})
            return Outcome(
                proposal_id=last.id,
                target=last.target,
                status="failed",
                applied=(),
                error="It changed since I changed it. Ask me to look again.",
                readback=current,
            )
        proposal = await self._propose(
            session_id=session_id,
            ability=last.ability,
            operation=f"undo:{last.writer_operation}",
            target=last.target,
            changes=inverse,
            rationale=f"undo {last.operation} ({last.id})",
            autonomy=Autonomy.ANNOUNCE,
            undo_of=last.id,
        )
        self._log(f"gate: undo {last.ability}.{last.operation} on {last.target}")
        outcome = await self._execute(proposal, now)
        if outcome.status == "success":
            self._store.set_status(last.id, ProposalStatus.UNDONE)
            self._store.event(last.id, "undone", {"by": proposal.id})
        return outcome

    # --- execution -----------------------------------------------------

    async def _execute(self, p: Proposal, now: datetime | None = None) -> Outcome:
        now = now or utcnow()
        writer = self.operation(p.ability, p.writer_operation).writer
        # Stale check: the target must not have changed since the snapshot.
        current = await writer.fetch(p.target)
        if current.version != p.snapshot.version:
            self._store.set_status(p.id, ProposalStatus.STALE)
            self._store.event(p.id, "stale", {"was": p.snapshot.version, "now": current.version})
            return Outcome(
                proposal_id=p.id,
                target=p.target,
                status="failed",
                applied=(),
                error="It changed since I read it. Ask me to look again.",
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
                await writer.apply(p.target, step, p.idempotency_key)
            except Exception as e:  # noqa: BLE001 - every failure is recorded, never hidden
                failed_step, error = label, str(e)
                self._store.event(p.id, "step_failed", {"index": i, "step": label, "error": error})
                break
            applied.append(label)
            self._store.event(p.id, "step_applied", {"index": i, "step": label})

        try:
            readback: Snapshot | None = await writer.fetch(p.target)
        except Exception as e:  # noqa: BLE001
            readback = None
            self._store.event(p.id, "readback_failed", {"error": str(e)})

        status = "success" if failed_step is None else ("partial" if applied else "failed")
        self._store.finish_operation(p.id, status, readback.version if readback else None)
        self._store.event(p.id, "finished", {"status": status})
        return Outcome(
            proposal_id=p.id,
            target=p.target,
            status=status,  # type: ignore[arg-type]
            applied=tuple(applied),
            failed_step=failed_step,
            error=error,
            readback=readback,
        )
