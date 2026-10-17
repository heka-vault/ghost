"""SQLite (WAL) persistence for proposals and the audit log (ADR 0009)."""

from __future__ import annotations

import json
import sqlite3
from datetime import datetime
from pathlib import Path

from ghost_domain.models import Proposal, ProposalStatus, utcnow

_SCHEMA = """
CREATE TABLE IF NOT EXISTS proposals (
    id          TEXT PRIMARY KEY,
    session_id  TEXT NOT NULL,
    status      TEXT NOT NULL,
    created_at  TEXT NOT NULL,
    expires_at  TEXT NOT NULL,
    body        TEXT NOT NULL
);
CREATE INDEX IF NOT EXISTS proposals_session ON proposals(session_id, status);

CREATE TABLE IF NOT EXISTS operations (
    proposal_id     TEXT PRIMARY KEY REFERENCES proposals(id),
    idempotency_key TEXT NOT NULL UNIQUE,
    started_at      TEXT NOT NULL,
    finished_at     TEXT,
    status          TEXT
);

CREATE TABLE IF NOT EXISTS operation_events (
    id          INTEGER PRIMARY KEY AUTOINCREMENT,
    proposal_id TEXT NOT NULL REFERENCES proposals(id),
    at          TEXT NOT NULL,
    event       TEXT NOT NULL,
    detail      TEXT
);

CREATE TABLE IF NOT EXISTS checkpoints (
    session_id  TEXT NOT NULL,
    key         TEXT NOT NULL,
    value       TEXT NOT NULL,
    updated_at  TEXT NOT NULL,
    PRIMARY KEY (session_id, key)
);
"""


class Store:
    def __init__(self, path: str | Path = ":memory:") -> None:
        self._conn = sqlite3.connect(str(path), isolation_level=None, check_same_thread=False)
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        self._conn.executescript(_SCHEMA)

    def close(self) -> None:
        self._conn.close()

    # proposals

    def insert_proposal(self, p: Proposal) -> None:
        self._conn.execute(
            "INSERT INTO proposals (id, session_id, status, created_at, expires_at, body)"
            " VALUES (?, ?, ?, ?, ?, ?)",
            (
                p.id,
                p.session_id,
                p.status.value,
                p.created_at.isoformat(),
                p.expires_at.isoformat(),
                p.model_dump_json(),
            ),
        )

    def get_proposal(self, proposal_id: str) -> Proposal | None:
        row = self._conn.execute(
            "SELECT body, status FROM proposals WHERE id = ?", (proposal_id,)
        ).fetchone()
        if row is None:
            return None
        p = Proposal.model_validate_json(row[0])
        return p.model_copy(update={"status": ProposalStatus(row[1])})

    def pending_for_session(self, session_id: str) -> Proposal | None:
        row = self._conn.execute(
            "SELECT body FROM proposals WHERE session_id = ? AND status = ?"
            " ORDER BY created_at DESC LIMIT 1",
            (session_id, ProposalStatus.PENDING.value),
        ).fetchone()
        return Proposal.model_validate_json(row[0]) if row else None

    def last_executed(self, session_id: str, since: datetime) -> Proposal | None:
        """The most recent executed, not-yet-undone proposal in this session since
        ``since``. Undo proposals themselves are skipped: undoing an undo is a new ask."""
        rows = self._conn.execute(
            "SELECT body FROM proposals WHERE session_id = ? AND status = ? AND created_at >= ?"
            " ORDER BY created_at DESC LIMIT 10",
            (session_id, ProposalStatus.EXECUTED.value, since.isoformat()),
        ).fetchall()
        for (body,) in rows:
            p = Proposal.model_validate_json(body)
            if p.undo_of is None:
                return p.model_copy(update={"status": ProposalStatus.EXECUTED})
        return None

    def set_status(self, proposal_id: str, status: ProposalStatus) -> None:
        self._conn.execute(
            "UPDATE proposals SET status = ? WHERE id = ?", (status.value, proposal_id)
        )

    def cancel_pending(
        self, session_id: str, status: ProposalStatus = ProposalStatus.CANCELLED
    ) -> int:
        cur = self._conn.execute(
            "UPDATE proposals SET status = ? WHERE session_id = ? AND status = ?",
            (status.value, session_id, ProposalStatus.PENDING.value),
        )
        return cur.rowcount

    def claim(self, proposal_id: str, now: datetime | None = None) -> bool:
        """Atomically move PENDING -> CLAIMED and open an operation row.

        Returns False if the proposal was not pending (already claimed, cancelled,
        or expired). The UNIQUE idempotency key makes a double claim impossible even
        across processes.
        """
        now = now or utcnow()
        with self._conn:
            self._conn.execute("BEGIN IMMEDIATE")
            cur = self._conn.execute(
                "UPDATE proposals SET status = ? WHERE id = ? AND status = ? AND expires_at > ?",
                (
                    ProposalStatus.CLAIMED.value,
                    proposal_id,
                    ProposalStatus.PENDING.value,
                    now.isoformat(),
                ),
            )
            if cur.rowcount != 1:
                self._conn.execute("ROLLBACK")
                return False
            self._conn.execute(
                "INSERT INTO operations (proposal_id, idempotency_key, started_at)"
                " VALUES (?, ?, ?)",
                (proposal_id, f"ghost:{proposal_id}", now.isoformat()),
            )
            self._conn.execute("COMMIT")
        return True

    def finish_operation(self, proposal_id: str, status: str) -> None:
        self._conn.execute(
            "UPDATE operations SET finished_at = ?, status = ? WHERE proposal_id = ?",
            (utcnow().isoformat(), status, proposal_id),
        )
        self.set_status(proposal_id, ProposalStatus.EXECUTED)

    # audit

    def event(self, proposal_id: str, event: str, detail: dict[str, object] | None = None) -> None:
        self._conn.execute(
            "INSERT INTO operation_events (proposal_id, at, event, detail) VALUES (?, ?, ?, ?)",
            (proposal_id, utcnow().isoformat(), event, json.dumps(detail) if detail else None),
        )

    def events(self, proposal_id: str) -> list[tuple[str, str, dict[str, object] | None]]:
        rows = self._conn.execute(
            "SELECT at, event, detail FROM operation_events WHERE proposal_id = ? ORDER BY id",
            (proposal_id,),
        ).fetchall()
        return [(at, ev, json.loads(d) if d else None) for at, ev, d in rows]

    # checkpoints

    def set_checkpoint(self, session_id: str, key: str, value: str) -> None:
        self._conn.execute(
            "INSERT INTO checkpoints (session_id, key, value, updated_at) VALUES (?, ?, ?, ?)"
            " ON CONFLICT(session_id, key) DO UPDATE SET value = excluded.value,"
            " updated_at = excluded.updated_at",
            (session_id, key, value, utcnow().isoformat()),
        )

    def get_checkpoint(self, session_id: str, key: str) -> str | None:
        row = self._conn.execute(
            "SELECT value FROM checkpoints WHERE session_id = ? AND key = ?", (session_id, key)
        ).fetchone()
        return row[0] if row else None
