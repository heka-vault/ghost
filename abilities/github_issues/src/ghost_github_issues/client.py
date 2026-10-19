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


def owner_of(full_name: str) -> str:
    return full_name.partition("/")[0].lower()


class GitHubClient:
    """``token`` is the default PAT (``GITHUB_TOKEN``); ``tokens_by_owner`` maps a
    repository owner (organisation or user, lowercase) to the PAT used for it (ADR
    0019). Both classic and fine-grained PATs are bearer tokens. Token values live
    only here and in the request headers; nothing logs or returns them."""

    def __init__(
        self,
        token: str,
        base_url: str = API,
        timeout: float = 10.0,
        *,
        tokens_by_owner: dict[str, str] | None = None,
    ) -> None:
        self._default = token
        self._by_owner = {k.lower(): v for k, v in (tokens_by_owner or {}).items() if v}
        self._http = httpx.AsyncClient(
            base_url=base_url,
            timeout=timeout,
            headers={
                "Accept": "application/vnd.github+json",
                "X-GitHub-Api-Version": "2022-11-28",
                "User-Agent": "ghost-voice-triage",
            },
        )

    def owners_with_token(self) -> list[str]:
        """Owners that have their own token (names only, for ``ghostd check``)."""
        return sorted(self._by_owner)

    def has_token_for(self, full_name: str) -> bool:
        return bool(self._by_owner.get(owner_of(full_name)) or self._default)

    def _auth(self, full_name: str) -> dict[str, str]:
        token = self._by_owner.get(owner_of(full_name)) or self._default
        if not token:
            raise ValueError(f"no GitHub token for {owner_of(full_name)}")
        return {"Authorization": f"Bearer {token}"}

    async def aclose(self) -> None:
        await self._http.aclose()

    async def _get(self, full_name: str, path: str, **params: Any) -> Any:
        r = await self._http.get(
            path,
            params={k: v for k, v in params.items() if v is not None},
            headers=self._auth(full_name),
        )
        r.raise_for_status()
        return r.json()

    async def _send(self, full_name: str, method: str, path: str, json: Any) -> Any:
        r = await self._http.request(method, path, json=json, headers=self._auth(full_name))
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
            full_name,
            f"/repos/{full_name}/issues",
            labels=labels,
            state=state,
            per_page=per_page,
            sort="updated",
            direction="desc",
        )
        return [i for i in items if "pull_request" not in i]

    async def get_issue(self, t: IssueTarget) -> dict[str, Any]:
        return await self._get(t.full_name, f"/repos/{t.full_name}/issues/{t.number}")

    async def list_comments(self, t: IssueTarget, per_page: int = 10) -> list[dict[str, Any]]:
        return await self._get(
            t.full_name, f"/repos/{t.full_name}/issues/{t.number}/comments", per_page=per_page
        )

    async def list_labels(self, full_name: str) -> list[str]:
        labels = await self._get(full_name, f"/repos/{full_name}/labels", per_page=100)
        return [lb["name"] for lb in labels]

    # --- writes (gate only) --------------------------------------------

    async def add_labels(self, t: IssueTarget, labels: list[str]) -> None:
        await self._send(
            t.full_name,
            "POST",
            f"/repos/{t.full_name}/issues/{t.number}/labels",
            {"labels": labels},
        )

    async def remove_label(self, t: IssueTarget, label: str) -> None:
        r = await self._http.delete(
            f"/repos/{t.full_name}/issues/{t.number}/labels/{label}",
            headers=self._auth(t.full_name),
        )
        if r.status_code != 404:  # already gone is fine: idempotent
            r.raise_for_status()

    async def create_comment(self, t: IssueTarget, body: str) -> None:
        await self._send(
            t.full_name,
            "POST",
            f"/repos/{t.full_name}/issues/{t.number}/comments",
            {"body": body},
        )

    async def set_state(self, t: IssueTarget, state: Literal["open", "closed"]) -> None:
        await self._send(
            t.full_name, "PATCH", f"/repos/{t.full_name}/issues/{t.number}", {"state": state}
        )


def parse_token_map(spec: str) -> dict[str, str]:
    """``GHOST_GITHUB_TOKENS="heka-vault=HEKA_ORG_TOKEN,quotient=QUOTIENT_TOKEN"`` ->
    owner (lowercase) -> the *name* of the environment variable holding its PAT."""
    out: dict[str, str] = {}
    for part in spec.split(","):
        part = part.strip()
        if not part:
            continue
        owner, sep, env_name = part.partition("=")
        owner, env_name = owner.strip().lower(), env_name.strip()
        if not sep or not owner or not env_name:
            raise ValueError(
                f"GHOST_GITHUB_TOKENS: {part!r} is not owner=ENV_NAME "
                "(each value names an environment variable holding that owner's token)"
            )
        out[owner] = env_name
    return out


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
