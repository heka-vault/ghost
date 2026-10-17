"""GitHub REST client: reads for the skill, writes for the gate (ADR 0008).

Writes exist only on ``GitHubIssueWriter``, which the gate owns. Nothing here is
exposed to the LLM as a tool.
"""

from __future__ import annotations

import re
from typing import Any, Literal

import httpx
from ghost_domain import Snapshot, Target

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

    async def get_issue(self, t: Target) -> dict[str, Any]:
        return await self._get(f"/repos/{t.full_name}/issues/{t.number}")

    async def list_comments(self, t: Target, per_page: int = 10) -> list[dict[str, Any]]:
        return await self._get(
            f"/repos/{t.full_name}/issues/{t.number}/comments", per_page=per_page
        )

    async def list_labels(self, full_name: str) -> list[str]:
        return [lb["name"] for lb in await self._get(f"/repos/{full_name}/labels", per_page=100)]

    # --- writes (gate only) --------------------------------------------

    async def add_labels(self, t: Target, labels: list[str]) -> None:
        await self._send(
            "POST", f"/repos/{t.full_name}/issues/{t.number}/labels", {"labels": labels}
        )

    async def remove_label(self, t: Target, label: str) -> None:
        r = await self._http.delete(f"/repos/{t.full_name}/issues/{t.number}/labels/{label}")
        if r.status_code != 404:  # already gone is fine: idempotent
            r.raise_for_status()

    async def create_comment(self, t: Target, body: str) -> None:
        await self._send("POST", f"/repos/{t.full_name}/issues/{t.number}/comments", {"body": body})

    async def set_state(self, t: Target, state: Literal["open", "closed"]) -> None:
        await self._send("PATCH", f"/repos/{t.full_name}/issues/{t.number}", {"state": state})

    async def delete_comment(self, t: Target, comment_id: int) -> None:
        r = await self._http.delete(f"/repos/{t.full_name}/issues/comments/{comment_id}")
        if r.status_code != 404:  # already gone is fine: idempotent
            r.raise_for_status()


def snapshot_of(issue: dict[str, Any]) -> Snapshot:
    return Snapshot(
        title=issue["title"],
        state=issue["state"],
        labels=tuple(sorted(lb["name"] for lb in issue.get("labels", []))),
        updated_at=issue["updated_at"],
    )


class GitHubIssueWriter:
    """Implements ``ghost_domain.IssueWriter`` over any client with ``GitHubClient``'s surface."""

    def __init__(self, client: Any) -> None:
        self._c = client

    async def fetch(self, target: Target) -> Snapshot:
        return snapshot_of(await self._c.get_issue(target))

    async def set_labels(
        self, target: Target, add: tuple[str, ...], remove: tuple[str, ...]
    ) -> None:
        for label in remove:
            await self._c.remove_label(target, label)
        if add:
            await self._c.add_labels(target, list(add))

    async def add_comment(self, target: Target, body: str, marker: str) -> None:
        # Dedupe on the hidden marker so a retry never posts twice.
        for c in await self._c.list_comments(target, per_page=100):
            if marker in (c.get("body") or ""):
                return
        await self._c.create_comment(target, f"{body}\n\n{marker_comment(marker)}")

    async def delete_comment(self, target: Target, marker: str) -> None:
        for c in await self._c.list_comments(target, per_page=100):
            if marker in (c.get("body") or "") and "id" in c:
                await self._c.delete_comment(target, int(c["id"]))

    async def set_state(self, target: Target, state: Literal["open", "closed"]) -> None:
        await self._c.set_state(target, state)
