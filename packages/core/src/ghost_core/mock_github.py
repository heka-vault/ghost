"""A pretend GitHub as an MCP server, for running Ghost before a token exists and for the
latency harness. Same tool shape as the official ``github-mcp-server`` issues toolset
(``issue_read`` and ``issue_write`` multiplexed by ``method``, ``add_issue_comment``,
``list_issues``, ``search_issues``), so the same declaration drives both.

Run it over stdio: ``ghost-mock-github --state PATH``. With a state file every write is
saved there and a restart picks it up again, so the audit log and the conversation history
outlive the process; delete the file to start over from the seed.
"""

from __future__ import annotations

import argparse
import json
import os
from datetime import UTC, datetime
from pathlib import Path
from typing import Any

MOCK_REPOS = ("ghost-mock/app", "ghost-mock/infra")

_SEED: list[tuple[str, int, str, list[str], str]] = [
    (
        "ghost-mock/app",
        142,
        "Choose Redis or Postgres for the session cache",
        ["needs-triage", "backend"],
        "We need a session cache before launch. Redis is simpler to operate; Postgres "
        "avoids a new service. Blocks #143 and #144.",
    ),
    (
        "ghost-mock/app",
        151,
        "Add dark mode toggle to settings",
        ["needs-triage", "frontend"],
        "Design is attached. Needs a decision on whether to follow the OS setting by default.",
    ),
    (
        "ghost-mock/infra",
        17,
        "Rotate the Discord webhook",
        ["needs-triage", "security"],
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

_COMMENTS: dict[str, list[dict[str, Any]]] = {
    "ghost-mock/app#142": [
        {
            "id": 1,
            "user": {"login": "dev"},
            "created_at": "2026-10-15T14:02:00Z",
            "body": "Leaning Redis unless ops objects.",
        }
    ]
}


def _now() -> str:
    return datetime.now(UTC).strftime("%Y-%m-%dT%H:%M:%SZ")


class MockGitHub:
    """The state behind the tools: issues and comments keyed ``owner/repo#number``."""

    def __init__(self, state_path: Path | None = None) -> None:
        self._path = state_path
        self.issues: dict[str, dict[str, Any]] = {}
        self.comments: dict[str, list[dict[str, Any]]] = {}
        self._next_comment_id = 1000
        if state_path is not None and state_path.exists():
            data = json.loads(state_path.read_text())
            self.issues = data["issues"]
            self.comments = data["comments"]
            self._next_comment_id = data["next_comment_id"]
            return
        for repo, number, title, labels, body in _SEED:
            key = f"{repo}#{number}"
            self.issues[key] = {
                "number": number,
                "title": title,
                "state": "open",
                "labels": [{"name": lb} for lb in labels],
                "body": body,
                "user": {"login": "dev"},
                "comments": len(_COMMENTS.get(key, [])),
                "updated_at": "2026-10-16T09:00:00Z",
                "html_url": f"https://github.com/{repo}/issues/{number}",
            }
            self.comments[key] = list(_COMMENTS.get(key, []))

    def save(self) -> None:
        if self._path is None:
            return
        data = {
            "issues": self.issues,
            "comments": self.comments,
            "next_comment_id": self._next_comment_id,
        }
        tmp = self._path.with_suffix(".tmp")
        tmp.write_text(json.dumps(data, indent=1))
        os.replace(tmp, self._path)

    def _key(self, owner: str, repo: str, number: int) -> str:
        key = f"{owner}/{repo}#{number}"
        if key not in self.issues:
            raise ValueError(f"{key} does not exist")
        return key

    # --- tools --------------------------------------------------------------------

    def issue_read(self, method: str, owner: str, repo: str, issue_number: int) -> dict[str, Any]:
        key = self._key(owner, repo, issue_number)
        if method == "get":
            return dict(self.issues[key])
        if method == "get_comments":
            return {"comments": self.comments[key][-10:]}
        if method == "get_labels":
            return {"labels": self.issues[key]["labels"]}
        raise ValueError(f"unknown method {method!r}")

    def issue_write(
        self,
        method: str,
        owner: str,
        repo: str,
        issue_number: int | None = None,
        title: str | None = None,
        body: str | None = None,
        labels: list[str] | None = None,
        state: str | None = None,
        state_reason: str | None = None,
    ) -> dict[str, Any]:
        if method == "create":
            number = max((i["number"] for i in self.issues.values()), default=0) + 1
            key = f"{owner}/{repo}#{number}"
            self.issues[key] = {
                "number": number,
                "title": title or "",
                "state": "open",
                "labels": [{"name": lb} for lb in labels or []],
                "body": body or "",
                "user": {"login": "operator"},
                "comments": 0,
                "updated_at": _now(),
                "html_url": f"https://github.com/{owner}/{repo}/issues/{number}",
            }
            self.comments[key] = []
            self.save()
            return dict(self.issues[key])
        if method != "update":
            raise ValueError(f"unknown method {method!r}")
        if issue_number is None:
            raise ValueError("issue_number is required for update")
        key = self._key(owner, repo, issue_number)
        issue = self.issues[key]
        if title is not None:
            issue["title"] = title
        if body is not None:
            issue["body"] = body
        if labels is not None:
            issue["labels"] = [{"name": lb} for lb in labels]
        if state is not None:
            if state not in ("open", "closed"):
                raise ValueError("state must be open or closed")
            issue["state"] = state
            issue["state_reason"] = state_reason if state == "closed" else None
        issue["updated_at"] = _now()
        self.save()
        return dict(issue)

    def add_issue_comment(
        self, owner: str, repo: str, issue_number: int, body: str
    ) -> dict[str, Any]:
        key = self._key(owner, repo, issue_number)
        self._next_comment_id += 1
        comment = {
            "id": self._next_comment_id,
            "user": {"login": "operator"},
            "created_at": _now(),
            "body": body,
        }
        self.comments[key].append(comment)
        self.issues[key]["comments"] += 1
        self.issues[key]["updated_at"] = _now()
        self.save()
        return comment

    def list_issues(
        self,
        owner: str,
        repo: str,
        state: str = "open",
        labels: list[str] | None = None,
        perPage: int = 30,  # noqa: N803 - the official server's spelling
    ) -> dict[str, Any]:
        want = set(labels or [])
        out = [
            i
            for key, i in self.issues.items()
            if key.startswith(f"{owner}/{repo}#")
            and (state == "all" or i["state"] == state)
            and want <= {lb["name"] for lb in i["labels"]}
        ]
        out.sort(key=lambda i: i["updated_at"], reverse=True)
        return {"issues": out[:perPage], "totalCount": len(out)}

    def search_issues(
        self, query: str, owner: str | None = None, repo: str | None = None
    ) -> dict[str, Any]:
        words = [w for w in query.lower().split() if ":" not in w]
        out = []
        for key, i in self.issues.items():
            if owner and not key.startswith(f"{owner}/"):
                continue
            if repo and f"/{repo}#" not in key:
                continue
            text = f"{i['title']} {i['body']}".lower()
            if all(w in text for w in words):
                out.append(i)
        return {"items": out[:30], "total_count": len(out)}


def build_server(state_path: Path | None = None):
    """The MCP server over ``MockGitHub``: tool names, argument names and read-only hints
    as the official server has them."""
    from mcp.server.mcpserver import MCPServer
    from mcp.types import ToolAnnotations

    gh = MockGitHub(state_path)
    srv = MCPServer("mock-github")
    read_only = ToolAnnotations(readOnlyHint=True)

    @srv.tool(annotations=read_only)
    def issue_read(method: str, owner: str, repo: str, issue_number: int) -> dict:
        """Get information about a specific issue in a GitHub repository (method: get,
        get_comments, get_labels)."""
        return gh.issue_read(method, owner, repo, issue_number)

    @srv.tool()
    def issue_write(
        method: str,
        owner: str,
        repo: str,
        issue_number: int | None = None,
        title: str | None = None,
        body: str | None = None,
        labels: list[str] | None = None,
        state: str | None = None,
        state_reason: str | None = None,
    ) -> dict:
        """Create a new or update an existing issue in a GitHub repository (method: create,
        update)."""
        return gh.issue_write(
            method, owner, repo, issue_number, title, body, labels, state, state_reason
        )

    @srv.tool()
    def add_issue_comment(owner: str, repo: str, issue_number: int, body: str) -> dict:
        """Add a comment to a specific issue in a GitHub repository."""
        return gh.add_issue_comment(owner, repo, issue_number, body)

    @srv.tool(annotations=read_only)
    def list_issues(
        owner: str,
        repo: str,
        state: str = "open",
        labels: list[str] | None = None,
        perPage: int = 30,  # noqa: N803
    ) -> dict:
        """List issues in a GitHub repository, newest first, optionally only those carrying
        every label given."""
        return gh.list_issues(owner, repo, state, labels, perPage)

    @srv.tool(annotations=read_only)
    def search_issues(query: str, owner: str | None = None, repo: str | None = None) -> dict:
        """Search for issues in GitHub repositories by words in their title or body."""
        return gh.search_issues(query, owner, repo)

    return srv


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description="A pretend GitHub, as an MCP server over stdio.")
    parser.add_argument("--state", type=Path, default=None, help="JSON file that keeps every write")
    args = parser.parse_args(argv)
    state = args.state.expanduser() if args.state else None
    if state is not None:
        state.parent.mkdir(parents=True, exist_ok=True)
    build_server(state).run(transport="stdio")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
