"""An in-memory GitHub for running Ghost before a PAT exists.

Same surface as ``GitHubClient`` so the skill and the gate run unchanged. State
lives for the process only. Seeded with a few issues in ``MOCK_REPOS``.
"""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Any, Literal

from ghost_domain import Target

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


class MockGitHubClient:
    def __init__(self) -> None:
        self._issues: dict[tuple[str, int], dict[str, Any]] = {}
        self._comments: dict[tuple[str, int], list[dict[str, Any]]] = {}
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
            self._comments[(repo, number)] = list(_COMMENTS.get((repo, number), []))

    async def aclose(self) -> None: ...

    def _issue(self, t: Target) -> dict[str, Any]:
        try:
            return self._issues[(t.full_name, t.number)]
        except KeyError:
            raise ValueError(f"{t} does not exist") from None

    def _touch(self, t: Target) -> None:
        self._issue(t)["updated_at"] = _now()

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
        self._comments[(t.full_name, t.number)].append(
            {"user": {"login": "operator"}, "created_at": _now(), "body": body}
        )
        self._touch(t)

    async def set_state(self, t: Target, state: Literal["open", "closed"]) -> None:
        self._issue(t)["state"] = state
        self._touch(t)
