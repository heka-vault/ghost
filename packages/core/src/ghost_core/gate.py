"""The write gate (ADR 0007, ADR 0011, ADR 0022).

Every write from every source comes through here. Sources (the lines, each MCP server's
declared writes) register their operations at startup; the operator's policy decides how
autonomous each one is; the gate does the same work at every level: the source's
allowlist, snapshot, bounded steps, idempotency, stale check, readback, audit row. At
``confirm`` a spoken yes sits between proposal and execution. At ``announce`` and
``quiet`` the write happens at once. Reversing a write is a new change through
``submit`` like any other.

A change at ``confirm`` waits on the waiting list (ADR 0030), not in the session: while
attached, the session's next turn answers it; set aside (a turn about something else, a
hang-up, a transfer), it waits for a yes by id until its time is up.
"""

from __future__ import annotations

import time
from collections.abc import Callable, Iterable, Mapping
from dataclasses import dataclass
from datetime import datetime, timedelta
from typing import Any

from ghost_core.confirmation import (
    ConfirmationClassifier,
    ConfirmationContext,
    RegexClassifier,
    Verdict,
)
from ghost_core.models import (
    PENDING_TTL,
    ChangeSet,
    Outcome,
    Proposal,
    ProposalStatus,
    Snapshot,
    Target,
    utcnow,
)
from ghost_core.policy import Autonomy, AutonomyPolicy, PolicyError
from ghost_core.request import asked_for
from ghost_core.scope import ActiveScope
from ghost_core.source import Operation
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


@dataclass(frozen=True)
class Classification:
    """Which classifier judged the last confirmation turn and how long it took. Timing
    only, for ghost_voice.latency; the words are never kept here."""

    classifier: str
    seconds: float


class WriteGate:
    """At most one pending proposal is attached to a session; any number wait set aside.

    * ``submit`` is callable from an LLM tool. Depending on policy it either writes
      now or stores a pending proposal.
    * ``on_user_turn`` is called by the pipeline with the operator's transcribed
      turn; ``confirm_by_request`` with the request that led to the proposal;
      ``confirm_pending`` and ``confirm_id`` with a click in the operator's own app.
      Those are the only paths from a pending proposal to ``execute``.
    """

    def __init__(
        self,
        store: Store,
        *,
        autonomy: AutonomyPolicy | None = None,
        classifier: ConfirmationClassifier | None = None,
        on_decision: Callable[[str], None] | None = None,
    ) -> None:
        self._store = store
        self._autonomy = autonomy or AutonomyPolicy()
        # ADR 0024: a department's sessions run under the file plus that department's own
        # levels (department.json), by slug; a department not here runs under the file.
        self._department_policies: dict[str, AutonomyPolicy] = {}
        self._classifier = classifier or RegexClassifier()
        self._log = on_decision or (lambda _msg: None)
        self._ops: dict[tuple[str, str], Operation] = {}
        # ADR 0024: a department sees a server it uses through its own declaration, so
        # its operations (allowlist, pinned arguments, bounds, asked_by) are registered
        # under the department and looked up there first; Ghost's are the fallback.
        self._department_ops: dict[str, dict[tuple[str, str], Operation]] = {}
        self._department_abilities: dict[str, set[str]] = {}  # sources a department owns
        self._departments: Any = None
        self.last_classification: Classification | None = None

    # --- registry ------------------------------------------------------

    def register(
        self, ability: str, operations: Iterable[Operation], *, department: str | None = None
    ) -> None:
        table = self._ops if department is None else self._department_ops.setdefault(department, {})
        if department is not None:
            self._department_abilities.setdefault(department, set()).add(ability)
        for op in operations:
            table[(ability, op.name)] = op

    def _lookup(self, ability: str, name: str, department: str | None) -> Operation | None:
        if department is not None:
            op = self._department_ops.get(department, {}).get((ability, name))
            if op is not None:
                return op
            if ability in self._department_abilities.get(department, ()):
                # The department sees this source through its own declaration: Ghost's
                # operation of the same name is never a stand-in for one it lacks.
                return None
        return self._ops.get((ability, name))

    def operation(self, ability: str, name: str, department: str | None = None) -> Operation:
        op = self._lookup(ability, name, department)
        if op is None:
            raise GateError(f"unknown operation {ability}.{name}")
        return op

    def operations(self, ability: str, department: str | None = None) -> list[Operation]:
        table: dict[tuple[str, str], Operation] = dict(self._ops)
        if department is not None:
            table.update(self._department_ops.get(department, {}))
        return [op for (a, _), op in table.items() if a == ability]

    @property
    def autonomy(self) -> AutonomyPolicy:
        return self._autonomy

    def set_autonomy(self, policy: AutonomyPolicy) -> None:
        """Swap the operator's policy live (the file was edited through the UI). The
        departments' levels are laid over the new file again."""
        self._autonomy = policy
        self._log(f"gate: autonomy policy reloaded from {policy.source}")
        self.refresh_departments()

    def attach_departments(self, departments: Any) -> None:
        """Follow the department folders (``ghost_core.departments.Departments``): their
        levels are rebuilt now and every time the registry re-reads the folders, so a
        ``department.json`` edited by hand applies at the next refresh."""
        self._departments = departments
        departments.on_refresh.append(self.refresh_departments)
        self.refresh_departments()

    def refresh_departments(self) -> None:
        if self._departments is None:
            return
        self._department_policies = {
            d.slug: d.policy(self._autonomy) for d in self._departments.list()
        }

    def set_department_policies(self, policies: Mapping[str, AutonomyPolicy]) -> None:
        """The merged policy per department (``Department.policy(base)``), replaced whole
        whenever the file or a department folder is re-read."""
        self._department_policies = dict(policies)

    @property
    def pending_ttl(self) -> timedelta:
        """How long a change at ``confirm`` waits (ADR 0030), from the policy file."""
        secs = self._autonomy.pending_ttl
        return timedelta(seconds=secs) if secs else PENDING_TTL

    def policy_for(self, department: str | None) -> AutonomyPolicy:
        if department is not None and department in self._department_policies:
            return self._department_policies[department]
        return self._autonomy

    def level(
        self,
        ability: str,
        operation: str,
        target: Target | None = None,
        department: str | None = None,
    ) -> Autonomy:
        op = self.operation(ability, operation, department)
        return self.policy_for(department).resolve(
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
        scope: ActiveScope | None = None,
        conversation: str | None = None,
        department: str | None = None,
    ) -> Submission:
        """``scope`` is the active agent's view for this source (ADR 0019): a target
        outside it is refused here, before policy or autonomy. None means unscoped.
        ``conversation`` is the agent the session is in and ``department`` where it
        works (ADR 0024), kept on the proposal for the audit; the department's own
        levels decide the autonomy."""
        op = self.operation(ability, operation, department)
        if not op.allowed(target):
            raise PolicyError(f"{target.spoken()} is not allowed for {ability}.{operation}")
        if scope is not None:
            scope.check_write(target.scope())  # raises OutOfScope, a PolicyError
        level = self.level(ability, operation, target, department=department)
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
            conversation=conversation,
            department=department,
        )
        self._log(f"gate: {ability}.{operation} on {target} -> {level.value}")
        if level is Autonomy.CONFIRM:
            return Submission(level, proposal, None)
        return Submission(level, proposal, await self._execute(proposal))

    async def apply_internal(
        self,
        *,
        session_id: str,
        ability: str,
        operation: str,
        target: Target,
        changes: ChangeSet,
        rationale: str,
    ) -> Outcome:
        """Ghost's own housekeeping (an automatic compaction, an edit made in the
        desktop app by the operator): applied at once at ``quiet`` whatever the policy
        says, still bounded, snapshotted and audited. Never reachable from a
        model tool; the policy's ``deny`` is honoured."""
        op = self.operation(ability, operation)
        if not op.allowed(target):
            raise PolicyError(f"{target.spoken()} is not allowed for {ability}.{operation}")
        if self.level(ability, operation, target) is Autonomy.DENY:
            raise PolicyError(f"{ability}.{operation} is not allowed on {target.scope()}")
        proposal = await self._propose(
            session_id=session_id,
            ability=ability,
            operation=operation,
            target=target,
            changes=changes,
            rationale=rationale,
            autonomy=Autonomy.QUIET,
        )
        self._log(f"gate: internal {ability}.{operation} on {target}")
        return await self._execute(proposal)

    async def prepare(
        self,
        *,
        session_id: str,
        ability: str,
        operation: str,
        target: Target,
        changes: ChangeSet,
        rationale: str,
        scope: ActiveScope | None = None,
    ) -> Proposal:
        """The confirm path regardless of policy. Writes nothing."""
        op = self.operation(ability, operation)
        if not op.allowed(target):
            raise PolicyError(f"{target.spoken()} is not allowed for {ability}.{operation}")
        if scope is not None:
            scope.check_write(target.scope())
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
        conversation: str | None = None,
        department: str | None = None,
    ) -> Proposal:
        writer = self.operation(ability, operation, department).writer
        # One attached at a time: a new one sets the old aside, on the waiting list.
        self._detach(session_id, "a newer change")
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
            conversation=conversation,
            department=department,
            expires_at=utcnow() + self.pending_ttl,
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
        """The operator dropped the change the session is waiting on ("never mind")."""
        p = self.pending(session_id)
        if p is None:
            return None
        self._store.set_status(p.id, ProposalStatus.CANCELLED)
        self._store.event(p.id, "cancelled", {"reason": reason})
        return p

    def set_aside(self, session_id: str, reason: str) -> Proposal | None:
        """The session moved on (a hang-up, a transfer, a turn about something else): the
        change it was waiting on stays pending, on the waiting list (ADR 0030)."""
        p = self.pending(session_id)
        if p is None:
            return None
        self._detach(session_id, reason)
        return p

    def _detach(self, session_id: str, reason: str) -> None:
        p = self._store.pending_for_session(session_id)
        if p is not None and self._store.detach(session_id):
            self._store.event(p.id, "set_aside", {"reason": reason})

    def waiting(self, now: datetime | None = None) -> list[Proposal]:
        """Every change waiting for a yes, oldest first; expired ones are marked so."""
        out = []
        for p in self._store.pending_proposals():
            if p.is_expired(now):
                self._store.set_status(p.id, ProposalStatus.EXPIRED)
                self._store.event(p.id, "expired")
                continue
            out.append(p)
        return out

    def waiting_one(self, proposal_id: str, now: datetime | None = None) -> Proposal | None:
        p = self._store.get_proposal(proposal_id)
        if p is None or p.status is not ProposalStatus.PENDING:
            return None
        if p.is_expired(now):
            self._store.set_status(p.id, ProposalStatus.EXPIRED)
            self._store.event(p.id, "expired")
            return None
        return p

    def bring_back(self, proposal_id: str, session_id: str) -> Proposal | None:
        """Attach a waiting change to this session, so its next turn answers it: Ghost
        reads it out and the operator says yes or no (ADR 0030). Writes nothing."""
        p = self.waiting_one(proposal_id)
        if p is None:
            return None
        if self._store.attach(p.id, session_id):
            self._store.event(p.id, "brought_back", {"session": session_id})
            return self._store.pending_for_session(session_id)
        return None

    def cancel_id(self, proposal_id: str, reason: str = "cancelled") -> Proposal | None:
        p = self.waiting_one(proposal_id)
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
        began = time.perf_counter()
        verdict = await self._classifier.classify(transcript, ctx)
        self.last_classification = Classification(
            classifier=str(
                getattr(self._classifier, "last_source", None)
                or getattr(self._classifier, "name", "custom")
            ),
            seconds=time.perf_counter() - began,
        )
        self._store.event(
            p.id,
            "user_turn",
            {
                "verdict": verdict.value,
                "text": transcript,
                "classifier": getattr(self._classifier, "name", "custom"),
            },
        )
        if verdict is Verdict.NEGATIVE:
            self._store.set_status(p.id, ProposalStatus.CANCELLED)
            self._store.event(p.id, "cancelled", {"reason": verdict.value})
            return verdict, None
        if verdict is not Verdict.AFFIRMATIVE:
            # Not an answer: the operator moved on. The change waits on the list (ADR 0030).
            self._detach(session_id, verdict.value)
            return verdict, None
        return verdict, await self._execute(p, now)

    async def confirm_by_request(
        self,
        session_id: str,
        request: str,
        *,
        question: str = "",
        now: datetime | None = None,
    ) -> Outcome | None:
        """The operator's own words that led to the pending proposal, from the pipeline's
        transcript. When they plainly asked for exactly this operation on this target
        (``ghost_core.request``), that is the yes: execute now. Otherwise None, and the
        proposal waits for a yes as before. ``question`` is Ghost's reply before the
        request, so "close it" can point at the issue it just named."""
        p = self.pending(session_id, now)
        if p is None:
            return None
        op = self._lookup(p.ability, p.operation, p.department)
        if op is None or not op.asked_by:
            return None
        if not asked_for(
            request, verbs=op.asked_by, mentions=p.target.mentions(), context=question
        ):
            return None
        self._store.event(
            p.id,
            "user_turn",
            {"verdict": Verdict.AFFIRMATIVE.value, "text": request, "classifier": "request"},
        )
        self._log(f"gate: the operator's request confirmed {p.id}")
        return await self._execute(p, now)

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

    async def confirm_id(
        self, proposal_id: str, *, source: str = "ui", now: datetime | None = None
    ) -> Outcome | None:
        """A click in the operator's own app on a change on the waiting list (ADR 0013,
        ADR 0030). The stale check in ``_execute`` still runs. None when it isn't waiting."""
        p = self.waiting_one(proposal_id, now)
        if p is None:
            return None
        self._store.event(
            p.id, "user_turn", {"verdict": Verdict.AFFIRMATIVE.value, "classifier": source}
        )
        self._log(f"gate: {source} confirmed {p.id} from the waiting list")
        return await self._execute(p, now)

    # --- execution -----------------------------------------------------

    async def _execute(self, p: Proposal, now: datetime | None = None) -> Outcome:
        now = now or utcnow()
        writer = self.operation(p.ability, p.operation, p.department).writer
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
