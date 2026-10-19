"""GitHub REST client: reads for the ability, writes for the gate (ADR 0008).

Writes exist only on ``GitHubIssueWriter``, which the gate owns. Nothing here is
exposed to the LLM as a tool.
"""

from __future__ import annotations

import re
from typing import Any, Literal

import httpx
from ghost_core import Change, Target

from ghost_github_issues.models import (
    AddComment,
    IssueSnapshot,
    IssueTarget,
    SetLabels,
    SetState,
)

API = "https://api.github.com"
MARKER_RE = re.compile(r"<!--\s*ghost:([0-9a-f]{32})\s*-->")


def marker_comment(marker: str) -> str:
    return f"<!-- {marker} -->"


class GitHubClient:
    def __init__(self, token: str, base_url: str = API, timeout: float = 10.0) -> None:
        self._http = httpx.AsyncClient(
            base_url=base_url,
            timeout=timeout,
            headers={
                "Authorization": f"Bearer {token}",
                "Accept": "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": "ghost-voice-triage",
            },
        )

    async def aclose(self) -> None:
        await self._http.aclose()

    async def _get(self, path: str, **params: Any) -> Any:
        r = await self._http.get(path, params={k: v for k, v in params.items() if v is not None})
        r.raise_for_status()
        return r.json()

    async def _send(self, method: str, path: str, json: Any) -> Any:
        r = await self._http.request(method, path, json=json)
        r.raise_for_status()
        return r.json() if r.content else None

    # --- reads ----------------------------------------------------------

    async def list_issues(
        self,
        full_name: str,
        *,
        labels: str | None = None,
        state: str = "open",
        per_page: int = 10,
    ) -> list[dict[str, Any]]:
        items = await self._get(
            f"/repos/{full_name}/issues",
            labels=labels,
            state=state,
            per_page=per_page,
            sort="updated",
            direction="desc",
        )
        return [i for i in items if "pull_request" not in i]

    async def get_issue(self, t: IssueTarget) -> dict[str, Any]:
        return await self._get(f"/repos/{t.full_name}/issues/{t.number}")

    async def list_comments(self, t: IssueTarget, per_page: int = 10) -> list[dict[str, Any]]:
        return await self._get(
            f"/repos/{t.full_name}/issues/{t.number}/comments", per_page=per_page
        )

    async def list_labels(self, full_name: str) -> list[str]:
        return [lb["name"] for lb in await self._get(f"/repos/{full_name}/labels", per_page=100)]

    # --- writes (gate only) --------------------------------------------

    async def add_labels(self, t: IssueTarget, labels: list[str]) -> None:
        await self._send(
            "POST", f"/repos/{t.full_name}/issues/{t.number}/labels", {"labels": labels}
        )

    async def remove_label(self, t: IssueTarget, label: str) -> None:
        r = await self._http.delete(f"/repos/{t.full_name}/issues/{t.number}/labels/{label}")
        if r.status_code != 404:  # already gone is fine: idempotent
            r.raise_for_status()

    async def create_comment(self, t: IssueTarget, body: str) -> None:
        await self._send("POST", f"/repos/{t.full_name}/issues/{t.number}/comments", {"body": body})

    async def set_state(self, t: IssueTarget, state: Literal["open", "closed"]) -> None:
        await self._send("PATCH", f"/repos/{t.full_name}/issues/{t.number}", {"state": state})


def snapshot_of(issue: dict[str, Any]) -> IssueSnapshot:
    return IssueSnapshot(
        version=issue["updated_at"],
        title=issue["title"],
        state=issue["state"],
        labels=tuple(sorted(lb["name"] for lb in issue.get("labels", []))),
    )


class GitHubIssueWriter:
    """Implements ``ghost_core.Writer`` over any client with ``GitHubClient``'s surface."""

    def __init__(self, client: Any) -> None:
        self._c = client

    async def fetch(self, target: Target) -> IssueSnapshot:
        assert isinstance(target, IssueTarget)
        return snapshot_of(await self._c.get_issue(target))

    async def apply(self, target: Target, change: Change, idempotency_key: str) -> None:
        assert isinstance(target, IssueTarget)
        match change:
            case SetLabels(add=add, remove=remove):
                for label in remove:
                    await self._c.remove_label(target, label)
                if add:
                    await self._c.add_labels(target, list(add))
            case AddComment(body=body):
                # Dedupe on the hidden marker so a retry never posts twice.
                for c in await self._c.list_comments(target, per_page=100):
                    if idempotency_key in (c.get("body") or ""):
                        return
                await self._c.create_comment(target, f"{body}\n\n{marker_comment(idempotency_key)}")
            case SetState(state=state):
                await self._c.set_state(target, state)
            case _:
                raise TypeError(f"GitHubIssueWriter cannot apply {type(change).__name__}")
