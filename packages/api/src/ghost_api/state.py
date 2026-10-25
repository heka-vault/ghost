"""What the API reads: the daemon's phase, the current session, recent turns, and a
fan-out of events to every connected client."""

from __future__ import annotations

import asyncio
import time
from collections import deque
from collections.abc import Callable
from datetime import UTC, datetime

from pydantic import TypeAdapter

from ghost_api.models import (
    ConversationEvent,
    Event,
    LogEvent,
    OutcomeEvent,
    OutcomeView,
    PendingEvent,
    PendingProposal,
    Phase,
    StatusEvent,
    Turn,
    TurnEvent,
)

_event_adapter: TypeAdapter[Event] = TypeAdapter(Event)


def now() -> datetime:
    return datetime.now(UTC)


class EventBus:
    """Each subscriber gets its own bounded queue; a slow client drops its oldest
    events rather than stalling the daemon."""

    def __init__(self, maxsize: int = 256) -> None:
        self._subs: set[asyncio.Queue[str]] = set()
        self._maxsize = maxsize

    def subscribe(self) -> asyncio.Queue[str]:
        q: asyncio.Queue[str] = asyncio.Queue(self._maxsize)
        self._subs.add(q)
        return q

    def unsubscribe(self, q: asyncio.Queue[str]) -> None:
        self._subs.discard(q)

    def publish(self, event: Event) -> None:
        payload = _event_adapter.dump_json(event).decode()
        for q in list(self._subs):
            if q.full():
                try:
                    q.get_nowait()
                except asyncio.QueueEmpty:
                    pass
            q.put_nowait(payload)

    @property
    def subscribers(self) -> int:
        return len(self._subs)


class DaemonState:
    def __init__(self, *, version: str, wake_mode: str) -> None:
        self.version = version
        self.wake_mode = wake_mode
        self.phase = Phase.STARTING
        self.session_id: str | None = None
        self.models_ready = False
        self.servers: list[str] = []
        self.pending: PendingProposal | None = None
        self.conversation: str | None = None
        self.department: str | None = None  # the department the session's agent works in
        self.session_origin: str | None = None  # "desk" | "phone" while a session runs
        self.ending = False  # going to sleep; the goodbye may still be playing
        self.turns: deque[Turn] = deque(maxlen=200)
        self.bus = EventBus()
        self._started = time.monotonic()
        self._status_provider: Callable[[], object] | None = None

    @property
    def uptime_secs(self) -> float:
        return time.monotonic() - self._started

    # --- mutations, each publishing an event ----------------------------------

    def set_phase(
        self, phase: Phase, session_id: str | None = None, origin: str | None = None
    ) -> None:
        self.phase = phase
        if phase in (Phase.PASSIVE, Phase.STARTING):
            self.session_id = None
            self.session_origin = None
            self.ending = False
            self.pending = None
            self.conversation = None
            self.department = None
            self.turns.clear()
        else:
            if session_id is not None:
                self.session_id = session_id
            if origin is not None:
                self.session_origin = origin
        self._publish_status()

    def set_ending(self) -> None:
        """The session is hanging up; it ends once the goodbye has played."""
        if self.session_id and not self.ending:
            self.ending = True
            self._publish_status()

    def set_models_ready(self, ready: bool) -> None:
        self.models_ready = ready
        self._publish_status()

    def add_turn(self, role: str, text: str, via: str = "voice") -> Turn | None:
        if not self.session_id or not text.strip():
            return None
        turn = Turn(role=role, text=text.strip(), session_id=self.session_id, at=now(), via=via)  # type: ignore[arg-type]
        self.turns.append(turn)
        self.bus.publish(TurnEvent(turn=turn))
        return turn

    def set_conversation(
        self, slug: str | None, changed: str | None = None, department: str | None = None
    ) -> None:
        self.conversation = slug if self.session_id else None
        if department is not None or slug is None:
            self.department = department if self.session_id else None
        self.bus.publish(ConversationEvent(active=self.conversation, changed=changed))
        self._publish_status()

    def set_pending(self, pending: PendingProposal | None) -> None:
        self.pending = pending
        self.bus.publish(PendingEvent(pending=pending))
        self._publish_status()

    def outcome(self, view: OutcomeView, via: str) -> None:
        self.bus.publish(OutcomeEvent(outcome=view, via=via))  # type: ignore[arg-type]

    def log(self, level: str, message: str) -> None:
        if self.bus.subscribers:
            self.bus.publish(LogEvent(level=level, message=message, at=now()))

    # --- status snapshot ----------------------------------------------------------

    def snapshot(self) -> object:
        """The current ``Status``, built by the provider the app installed."""
        if self._status_provider is None:
            raise RuntimeError("no status provider installed")
        return self._status_provider()

    def set_status_provider(self, provider: Callable[[], object]) -> None:
        """The app installs a provider that builds a full ``Status`` from this state plus
        the gate's pending proposal, so one function owns the shape."""
        self._status_provider = provider

    def _publish_status(self) -> None:
        if self._status_provider is not None:
            self.bus.publish(StatusEvent(status=self._status_provider()))  # type: ignore[arg-type]
