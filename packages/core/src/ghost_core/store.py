"""SQLite (WAL) persistence for proposals, the audit log (ADR 0009), and what Ghost
writes about agents: the registry mirror, the transcript, every compaction (ADR 0014,
ADR 0024). Transcripts and compactions are keyed by the agent's stable id, so an agent
folder moved by hand keeps its history."""

from __future__ import annotations

import json
import sqlite3
from datetime import datetime
from pathlib import Path
from typing import TYPE_CHECKING

from loguru import logger

from ghost_core.codec import dump_proposal, load_proposal
from ghost_core.models import Proposal, ProposalStatus, utcnow

if TYPE_CHECKING:
    from ghost_core.conversations.models import Compaction, Profile, TurnRecord

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
    status          TEXT,
    readback_version TEXT
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

-- Agents (ADR 0014, ADR 0024): a mirror of each agent folder plus what only Ghost
-- writes (timestamps, retired), the transcript, every compaction version. ``conversation``
-- columns hold the agent's id.
CREATE TABLE IF NOT EXISTS conversations (
    id              TEXT PRIMARY KEY,
    slug            TEXT NOT NULL UNIQUE,
    department      TEXT NOT NULL,
    name            TEXT NOT NULL,
    aliases         TEXT NOT NULL DEFAULT '[]',
    job             TEXT NOT NULL DEFAULT '',
    voice           TEXT,
    speed           REAL,
    created_at      TEXT NOT NULL,
    updated_at      TEXT NOT NULL,
    last_active_at  TEXT,
    retired_at      TEXT,
    deleted_at      TEXT
);

CREATE TABLE IF NOT EXISTS turns (
    id              INTEGER PRIMARY KEY AUTOINCREMENT,
    conversation    TEXT NOT NULL,
    session_id      TEXT NOT NULL,
    role            TEXT NOT NULL,
    text            TEXT NOT NULL,
    at              TEXT NOT NULL,
    compaction_id   TEXT,
    hidden_by       TEXT  -- the proposal marker of the reset/delete that hid it
);
CREATE INDEX IF NOT EXISTS turns_conversation ON turns(conversation, id);

CREATE TABLE IF NOT EXISTS compactions (
    id              TEXT PRIMARY KEY,
    conversation    TEXT NOT NULL,
    summary         TEXT NOT NULL,
    through_turn_id INTEGER,
    at              TEXT NOT NULL,
    source          TEXT NOT NULL DEFAULT 'auto'
);
CREATE INDEX IF NOT EXISTS compactions_conversation ON compactions(conversation, at);
"""

# The three tables the lines (ADR 0014 to 0023) lived in. ADR 0024 starts over: they are
# moved aside under these names, never read again, for the operator to drop.
_LINE_TABLES = ("conversations", "turns", "compactions")

# Statuses older databases may hold. Ghost once had an undo feature that marked a
# reversed proposal "undone"; the write did happen, and its reversal is its own row
# (operation ``undo:<op>``), so it reads back as executed.
_LEGACY_STATUS = {"undone": ProposalStatus.EXECUTED}


def _status(raw: str) -> ProposalStatus:
    return _LEGACY_STATUS.get(raw) or ProposalStatus(raw)


class Store:
    def __init__(self, path: str | Path = ":memory:") -> None:
        self._conn = sqlite3.connect(str(path), isolation_level=None, check_same_thread=False)
        self._conn.execute("PRAGMA journal_mode=WAL")
        self._conn.execute("PRAGMA foreign_keys=ON")
        self._retire_line_tables()
        self._conn.executescript(_SCHEMA)
        self._migrate()

    def _retire_line_tables(self) -> None:
        """A database from before ADR 0024 keeps its lines in tables keyed by slug. They
        are renamed ``retired_*`` so the new layout starts empty, as the ADR says, and
        nothing of the operator's is deleted by code."""
        cols = {r[1] for r in self._conn.execute("PRAGMA table_info(conversations)")}
        if not cols or "id" in cols:
            return
        stamp = utcnow().strftime("%Y%m%d")
        for table in _LINE_TABLES:
            self._conn.execute(f"ALTER TABLE {table} RENAME TO retired_{table}_{stamp}")
        for index in ("turns_conversation", "compactions_conversation"):
            self._conn.execute(f"DROP INDEX IF EXISTS {index}")
        logger.warning(
            "store: the lines of the earlier layout were moved aside as retired_* tables "
            "(ADR 0024); agents start fresh"
        )

    def _migrate(self) -> None:
        cols = {r[1] for r in self._conn.execute("PRAGMA table_info(operations)")}
        if "readback_version" not in cols:
            self._conn.execute("ALTER TABLE operations ADD COLUMN readback_version TEXT")

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
                dump_proposal(p),
            ),
        )

    def get_proposal(self, proposal_id: str) -> Proposal | None:
        row = self._conn.execute(
            "SELECT body, status FROM proposals WHERE id = ?", (proposal_id,)
        ).fetchone()
        if row is None:
            return None
        p = load_proposal(row[0])
        return p.model_copy(update={"status": _status(row[1])})

    def pending_for_session(self, session_id: str) -> Proposal | None:
        row = self._conn.execute(
            "SELECT body FROM proposals WHERE session_id = ? AND status = ?"
            " ORDER BY created_at DESC LIMIT 1",
            (session_id, ProposalStatus.PENDING.value),
        ).fetchone()
        return load_proposal(row[0]) if row else None

    def list_proposals(
        self,
        *,
        limit: int = 50,
        offset: int = 0,
        ability: str | None = None,
        status: str | None = None,
        session_id: str | None = None,
        department: str | None = None,
        conversation: str | None = None,
        executed_only: bool = False,
    ) -> tuple[list[Proposal], int]:
        """Newest first, for the audit view and a department's journal. Rows from an
        older schema are skipped."""
        where, args = [], []
        if status:
            raw = [status, *(k for k, v in _LEGACY_STATUS.items() if v.value == status)]
            where.append(f"status IN ({', '.join('?' * len(raw))})")
            args.extend(raw)
        if executed_only:
            where.append("status = ?")
            args.append(ProposalStatus.EXECUTED.value)
        if session_id:
            where.append("session_id = ?")
            args.append(session_id)
        if ability:
            where.append("json_extract(body, '$.ability') = ?")
            args.append(ability)
        if department:
            where.append("json_extract(body, '$.department') = ?")
            args.append(department)
        if conversation:
            where.append("json_extract(body, '$.conversation') = ?")
            args.append(conversation)
        clause = (" WHERE " + " AND ".join(where)) if where else ""
        total = self._conn.execute(f"SELECT COUNT(*) FROM proposals{clause}", args).fetchone()[0]
        rows = self._conn.execute(
            f"SELECT body, status FROM proposals{clause} ORDER BY created_at DESC LIMIT ? OFFSET ?",
            [*args, limit, offset],
        ).fetchall()
        out = []
        for body, st in rows:
            try:
                out.append(load_proposal(body).model_copy(update={"status": _status(st)}))
            except Exception:  # noqa: BLE001
                continue
        return out, int(total)

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

    def finish_operation(
        self, proposal_id: str, status: str, readback_version: str | None = None
    ) -> None:
        self._conn.execute(
            "UPDATE operations SET finished_at = ?, status = ?, readback_version = ?"
            " WHERE proposal_id = ?",
            (utcnow().isoformat(), status, readback_version, proposal_id),
        )
        self.set_status(proposal_id, ProposalStatus.EXECUTED)

    def readback_version(self, proposal_id: str) -> str | None:
        row = self._conn.execute(
            "SELECT readback_version FROM operations WHERE proposal_id = ?", (proposal_id,)
        ).fetchone()
        return row[0] if row else None

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

    # per-ability key/value

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

    # agents (ADR 0014, ADR 0024)

    def upsert_conversation(self, p: Profile) -> None:
        self._conn.execute(
            "INSERT INTO conversations (id, slug, department, name, aliases, job, voice, speed,"
            " created_at, updated_at, last_active_at, retired_at, deleted_at)"
            " VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)"
            " ON CONFLICT(id) DO UPDATE SET slug = excluded.slug,"
            " department = excluded.department, name = excluded.name,"
            " aliases = excluded.aliases, job = excluded.job, voice = excluded.voice,"
            " speed = excluded.speed, updated_at = excluded.updated_at,"
            " last_active_at = excluded.last_active_at, retired_at = excluded.retired_at,"
            " deleted_at = excluded.deleted_at",
            (
                p.id,
                p.slug,
                p.department,
                p.name,
                json.dumps(list(p.aliases)),
                p.job,
                p.voice,
                p.speed,
                p.created_at.isoformat(),
                p.updated_at.isoformat(),
                p.last_active_at.isoformat() if p.last_active_at else None,
                p.retired_at.isoformat() if p.retired_at else None,
                p.deleted_at.isoformat() if p.deleted_at else None,
            ),
        )

    def get_conversation(self, slug: str, *, include_deleted: bool = False) -> Profile | None:
        row = self._conn.execute(
            f"SELECT {_PROFILE_COLS} FROM conversations WHERE slug = ?", (slug,)
        ).fetchone()
        if row is None:
            return None
        p = _profile(row)
        if p.deleted_at is not None and not include_deleted:
            return None
        return p

    def get_conversation_by_id(self, id: str) -> Profile | None:
        row = self._conn.execute(
            f"SELECT {_PROFILE_COLS} FROM conversations WHERE id = ?", (id,)
        ).fetchone()
        return _profile(row) if row else None

    def list_conversations(
        self, *, include_retired: bool = False, include_deleted: bool = False
    ) -> list[Profile]:
        where = ["1 = 1" if include_deleted else "deleted_at IS NULL"]
        if not include_retired:
            where.append("retired_at IS NULL")
        rows = self._conn.execute(
            f"SELECT {_PROFILE_COLS} FROM conversations WHERE {' AND '.join(where)}"
            " ORDER BY CASE WHEN slug = 'ghost' THEN 0 ELSE 1 END, lower(name)"
        ).fetchall()
        return [_profile(r) for r in rows]

    def delete_conversation_row(self, id: str) -> None:
        """Forget a mirror row whose folder is gone and whose history is empty."""
        self._conn.execute("DELETE FROM conversations WHERE id = ?", (id,))

    def touch_conversation(self, id: str) -> None:
        self._conn.execute(
            "UPDATE conversations SET last_active_at = ? WHERE id = ?",
            (utcnow().isoformat(), id),
        )

    def add_turn(self, conversation: str, session_id: str, role: str, text: str) -> int:
        cur = self._conn.execute(
            "INSERT INTO turns (conversation, session_id, role, text, at) VALUES (?, ?, ?, ?, ?)",
            (conversation, session_id, role, text, utcnow().isoformat()),
        )
        return int(cur.lastrowid or 0)

    def recent_turns(self, conversation: str, *, limit: int, uncompacted: bool) -> list[TurnRecord]:
        """Oldest first. ``uncompacted`` restricts to turns no summary covers yet."""
        where = "conversation = ? AND hidden_by IS NULL"
        if uncompacted:
            where += " AND compaction_id IS NULL"
        rows = self._conn.execute(
            f"SELECT id, conversation, session_id, role, text, at, compaction_id FROM turns"
            f" WHERE {where} ORDER BY id DESC LIMIT ?",
            (conversation, limit),
        ).fetchall()
        return [_turn(r) for r in reversed(rows)]

    def move_session_turns(self, session_id: str, src: str, dst: str) -> int:
        """Carry this session's turns from one agent to another (save-as, a split move)."""
        cur = self._conn.execute(
            "UPDATE turns SET conversation = ? WHERE session_id = ? AND conversation = ?"
            " AND hidden_by IS NULL AND compaction_id IS NULL",
            (dst, session_id, src),
        )
        return cur.rowcount

    def turn_count(self, conversation: str) -> int:
        return int(
            self._conn.execute(
                "SELECT COUNT(*) FROM turns WHERE conversation = ? AND hidden_by IS NULL",
                (conversation,),
            ).fetchone()[0]
        )

    def last_turn_id(self, conversation: str) -> int | None:
        row = self._conn.execute(
            "SELECT MAX(id) FROM turns WHERE conversation = ? AND hidden_by IS NULL",
            (conversation,),
        ).fetchone()
        return int(row[0]) if row and row[0] is not None else None

    def mark_compacted(self, conversation: str, through_turn_id: int, compaction_id: str) -> int:
        cur = self._conn.execute(
            "UPDATE turns SET compaction_id = ? WHERE conversation = ? AND id <= ?"
            " AND compaction_id IS NULL AND hidden_by IS NULL",
            (compaction_id, conversation, through_turn_id),
        )
        return cur.rowcount

    def tag_turns(self, conversation: str, marker: str) -> int:
        """Hide every turn (a reset or delete). Hidden turns are pruned with the rest."""
        cur = self._conn.execute(
            "UPDATE turns SET hidden_by = ? WHERE conversation = ? AND hidden_by IS NULL",
            (marker, conversation),
        )
        return cur.rowcount

    def prune_turns(self, before: datetime) -> int:
        """Summarized or hidden turns older than ``before`` are dropped; the summary
        stands in for them."""
        cur = self._conn.execute(
            "DELETE FROM turns WHERE at < ?"
            " AND (compaction_id IS NOT NULL OR hidden_by IS NOT NULL)",
            (before.isoformat(),),
        )
        return cur.rowcount

    def add_compaction(self, c: Compaction) -> None:
        self._conn.execute(
            "INSERT INTO compactions (id, conversation, summary, through_turn_id, at, source)"
            " VALUES (?, ?, ?, ?, ?, ?)",
            (c.id, c.conversation, c.summary, c.through_turn_id, c.at.isoformat(), c.source),
        )

    def compactions(self, conversation: str, *, limit: int = 50) -> list[Compaction]:
        from ghost_core.conversations.models import Compaction

        rows = self._conn.execute(
            "SELECT id, conversation, summary, through_turn_id, at, source FROM compactions"
            " WHERE conversation = ? ORDER BY at DESC LIMIT ?",
            (conversation, limit),
        ).fetchall()
        return [
            Compaction(
                id=r[0],
                conversation=r[1],
                summary=r[2],
                through_turn_id=r[3],
                at=datetime.fromisoformat(r[4]),
                source=r[5],
            )
            for r in rows
        ]


_PROFILE_COLS = (
    "id, slug, department, name, aliases, job, voice, speed, created_at, updated_at,"
    " last_active_at, retired_at, deleted_at"
)


def _profile(row) -> Profile:
    from ghost_core.conversations.models import Profile

    return Profile(
        id=row[0],
        slug=row[1],
        department=row[2],
        name=row[3],
        aliases=tuple(json.loads(row[4] or "[]")),
        job=row[5] or "",
        voice=row[6],
        speed=float(row[7]) if row[7] is not None else None,
        created_at=datetime.fromisoformat(row[8]),
        updated_at=datetime.fromisoformat(row[9]),
        last_active_at=datetime.fromisoformat(row[10]) if row[10] else None,
        retired_at=datetime.fromisoformat(row[11]) if row[11] else None,
        deleted_at=datetime.fromisoformat(row[12]) if row[12] else None,
    )


def _turn(row) -> TurnRecord:
    from ghost_core.conversations.models import TurnRecord

    return TurnRecord(
        id=row[0],
        conversation=row[1],
        session_id=row[2],
        role=row[3],
        text=row[4],
        at=datetime.fromisoformat(row[5]),
        compaction_id=row[6],
    )
