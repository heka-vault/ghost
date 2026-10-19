"""A pretend GitHub for running Ghost before a PAT exists.

Same surface as ``GitHubClient`` so the ability and the gate run unchanged. Seeded with a
few issues in ``MOCK_REPOS``. With a ``state_path`` every write is saved there and a
restart picks it up again: the audit log and the conversation history outlive the
process, and a mock that forgot its changes made Ghost's own past work look invented.
Delete the file to start over from the seed.
"""

from __future__ import annotations

import json
import os
from datetime import UTC, datetime
from pathlib import Path
from typing import Any, Literal

from ghost_github_issues.models import IssueTarget as Target

MOCK_REPOS = "ghost-mock/app,ghost-mock/infra"

_SEED: list[tuple[str, int, str, list[str], str]] = [
    (
        "ghost-mock/app",
        142,
        "Choose Redis or Postgres for the session cache",
        ["needs-signoff", "backend"],
        "We need a session cache before launch. Redis is simpler to operate; Postgres "
        "avoids a new service. Blocks #143 and #144.",
    ),
    (
        "ghost-mock/app",
        151,
        "Add dark mode toggle to settings",
        ["needs-signoff", "frontend"],
        "Design is attached. Needs a decision on whether to follow the OS setting by default.",
    ),
    (
        "ghost-mock/infra",
        17,
        "Rotate the Discord webhook",
        ["needs-signoff", "security"],
        "The URL was printed into a session log. Create a new webhook and update the config.",
    ),
    (
        "ghost-mock/app",
        98,
        "Flaky test in payments suite",
        ["bug"],
        "test_refund_partial fails about one run in ten.",
    ),
]

_COMMENTS: dict[tuple[str, int], list[dict[str, Any]]] = {
    ("ghost-mock/app", 142): [
        {
            "user": {"login": "dev"},
            "created_at": "2026-10-15T14:02:00Z",
            "body": "Leaning Redis unless ops objects.",
        }
    ]
}


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


def _key(repo: str, number: int) -> str:
    return f"{repo}#{number}"


def _unkey(key: str) -> tuple[str, int]:
    repo, _, number = key.rpartition("#")
    return repo, int(number)


class MockGitHubClient:
    def __init__(self, state_path: Path | None = None) -> None:
        self._issues: dict[tuple[str, int], dict[str, Any]] = {}
        self._comments: dict[tuple[str, int], list[dict[str, Any]]] = {}
        self._next_comment_id = 1000
        self._path = state_path
        if state_path is not None and state_path.exists():
            self._load(state_path)
            return
        for repo, number, title, labels, body in _SEED:
            self._issues[(repo, number)] = {
                "number": number,
                "title": title,
                "state": "open",
                "labels": [{"name": lb} for lb in labels],
                "body": body,
                "user": {"login": "dev"},
                "comments": len(_COMMENTS.get((repo, number), [])),
                "updated_at": "2026-10-16T09:00:00Z",
            }
            self._comments[(repo, number)] = [
                {"id": 1 + k, **c} for k, c in enumerate(_COMMENTS.get((repo, number), []))
            ]

    def _load(self, path: Path) -> None:
        data = json.loads(path.read_text())
        self._issues = {_unkey(k): v for k, v in data["issues"].items()}
        self._comments = {_unkey(k): v for k, v in data["comments"].items()}
        self._next_comment_id = data["next_comment_id"]

    def _save(self) -> None:
        if self._path is None:
            return
        data = {
            "issues": {_key(*k): v for k, v in self._issues.items()},
            "comments": {_key(*k): v for k, v in self._comments.items()},
            "next_comment_id": self._next_comment_id,
        }
        tmp = self._path.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, indent=1))
        os.replace(tmp, self._path)

    async def aclose(self) -> None: ...

    def _issue(self, t: Target) -> dict[str, Any]:
        try:
            return self._issues[(t.full_name, t.number)]
        except KeyError:
            raise ValueError(f"{t} does not exist") from None

    def _touch(self, t: Target) -> None:
        """Every write ends here: stamp the issue and save."""
        self._issue(t)["updated_at"] = _now()
        self._save()

    # reads

    async def list_issues(
        self, full_name: str, *, labels: str | None = None, state: str = "open", per_page: int = 10
    ) -> list[dict[str, Any]]:
        want = {s.strip() for s in labels.split(",")} if labels else set()
        out = [
            i
            for (repo, _), i in self._issues.items()
            if repo == full_name
            and (state == "all" or i["state"] == state)
            and want <= {lb["name"] for lb in i["labels"]}
        ]
        out.sort(key=lambda i: i["updated_at"], reverse=True)
        return out[:per_page]

    async def get_issue(self, t: Target) -> dict[str, Any]:
        return dict(self._issue(t))

    async def list_comments(self, t: Target, per_page: int = 10) -> list[dict[str, Any]]:
        self._issue(t)
        return self._comments[(t.full_name, t.number)][-per_page:]

    async def list_labels(self, full_name: str) -> list[str]:
        return sorted(
            {
                lb["name"]
                for (r, _), i in self._issues.items()
                if r == full_name
                for lb in i["labels"]
            }
        )

    # writes (gate only)

    async def add_labels(self, t: Target, labels: list[str]) -> None:
        i = self._issue(t)
        have = {lb["name"] for lb in i["labels"]}
        i["labels"] += [{"name": lb} for lb in labels if lb not in have]
        self._touch(t)

    async def remove_label(self, t: Target, label: str) -> None:
        i = self._issue(t)
        i["labels"] = [lb for lb in i["labels"] if lb["name"] != label]
        self._touch(t)

    async def create_comment(self, t: Target, body: str) -> None:
        self._issue(t)["comments"] += 1
        self._next_comment_id += 1
        self._comments[(t.full_name, t.number)].append(
            {
                "id": self._next_comment_id,
                "user": {"login": "operator"},
                "created_at": _now(),
                "body": body,
            }
        )
        self._touch(t)

    async def set_state(self, t: Target, state: Literal["open", "closed"]) -> None:
        self._issue(t)["state"] = state
        self._touch(t)
