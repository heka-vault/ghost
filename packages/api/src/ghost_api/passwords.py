"""Password routes (ADR 0036): Ghost's vault, as the Passwords page sees it.

A password goes in and never comes back out. ``GET /passwords`` lists names and what may
be said about them (set or not, what uses it, when it was last used); ``PUT`` saves one,
``DELETE`` forgets one, ``POST /passwords/import`` moves the plaintext copies into the
vault once, and ``POST /passwords/unlock`` asks the keyring to unlock. No route returns a
value, and none is logged. The model has no route here.
"""

from __future__ import annotations

import asyncio
from collections.abc import Iterable
from pathlib import Path
from typing import Any

from fastapi import FastAPI, HTTPException
from ghost_core.envfile import read_env
from ghost_core.password_import import import_passwords, sources
from ghost_core.passwords import GHOST_USES, Passwords, VaultClosed, check_name, check_value
from loguru import logger

from ghost_api.models import (
    ImportSourceView,
    PasswordImportResult,
    PasswordInfo,
    PasswordResult,
    PasswordSet,
    PasswordsView,
    VaultView,
)


def tilde(p: Path | str) -> str:
    home, s = str(Path.home()), str(p)
    return "~" + s[len(home) :] if s == home or s.startswith(home + "/") else s


def listed_names(settings: dict[str, str]) -> set[str]:
    """Names the operator listed in settings.env as passwords their own config uses
    (``GHOST_APP_SECRETS``; ``GHOST_MCP_SECRETS`` is its older name)."""
    raw = (
        (settings.get("GHOST_APP_SECRETS") or "") + "," + (settings.get("GHOST_MCP_SECRETS") or "")
    )
    return {n.strip() for n in raw.split(",") if n.strip() and n.strip().isupper()}


def used_by(deps: Any) -> dict[str, list[str]]:
    """What uses each password, in the operator's words: Ghost itself, an app, a webhook."""
    out: dict[str, list[str]] = {}
    for name, what in GHOST_USES.items():
        if name != "GITHUB_TOKEN":  # an app's, when one names it
            out.setdefault(name, []).append(what)
    if deps.apps is not None:
        for spec in deps.apps.specs.values():
            for name in spec.secrets():
                out.setdefault(name, []).append(spec.shown)
    folder = getattr(deps, "folder", None)
    for hook in getattr(folder, "hooks", {}).values() if folder is not None else ():
        out.setdefault(hook.secret_name(), []).append(f"Webhook {hook.name}")
    for name in listed_names(read_env(deps.settings_path)):
        out.setdefault(name, [])
    return out


def referenced(deps: Any) -> set[str]:
    """Every name something in Ghost refers to (its own, the apps', the webhooks')."""
    return set(used_by(deps)) | {"GITHUB_TOKEN"}


def passwords_view(deps: Any) -> PasswordsView:
    vault: Passwords | None = deps.passwords
    if vault is None:
        raise HTTPException(503, "the vault is not available")
    uses = used_by(deps)
    meta = {m.name: m for m in vault.info()} if vault.is_open else {}
    names = [*GHOST_USES, *sorted((set(uses) | set(meta)) - set(GHOST_USES))]
    rows = []
    for name in names:
        m = meta.get(name)
        rows.append(
            PasswordInfo(
                name=name,
                set=m is not None,
                used_by=uses.get(name, []),
                ghost=name in GHOST_USES and name != "GITHUB_TOKEN",
                added=m.added if m else None,
                updated=m.updated if m else None,
                last_used=m.last_used if m else None,
            )
        )
    return PasswordsView(
        vault=VaultView(state=vault.state, problem=vault.problem, path=tilde(vault.path)),
        passwords=rows,
        importable=[
            ImportSourceView(kind=s.kind, path=tilde(s.path))  # type: ignore[arg-type]
            for s in sources(deps.env_path.parent, deps.vault_env_path)
        ],
    )


def secret_states(deps: Any, names: Iterable[str]) -> list[tuple[str, bool]]:
    """Set or not, for the Settings view's list."""
    vault: Passwords | None = deps.passwords
    held = set(vault.names()) if vault is not None and vault.is_open else set()
    return [(n, n in held) for n in names]


async def revive_apps_using(deps: Any, names: Iterable[str]) -> None:
    """An app that couldn't connect for want of one of these tries again now."""
    if deps.apps is None:
        return
    wanted = set(names)
    for spec in list(deps.apps.specs.values()):
        if wanted & set(spec.secrets()) and not deps.apps.is_up(spec.name):
            try:
                if await deps.apps.revive(spec.name):
                    deps.state.publish_apps(changed=spec.name)
            except Exception as e:  # noqa: BLE001 - the app's own trouble; it stays listed
                logger.warning(f"passwords: {spec.name} didn't connect ({type(e).__name__})")


def save_passwords(deps: Any, values: dict[str, str | None]) -> bool:
    """Into the vault (None forgets one). True when one of them is Ghost's own, which it
    picks up at its next start."""
    vault: Passwords | None = deps.passwords
    if vault is None:
        raise HTTPException(503, "the vault is not available")
    put = {k: v.strip() for k, v in values.items() if v is not None}
    gone = [k for k, v in values.items() if v is None]
    try:
        for k, v in put.items():
            check_name(k)
            check_value(v)
        if put:
            vault.set_many(put)
        if gone:
            vault.delete(*gone)
    except ValueError as e:
        raise HTTPException(422, str(e)) from None
    except VaultClosed as e:
        raise HTTPException(423, f"{e}. Unlock it, then try again.") from None
    logger.info(f"passwords: saved {sorted(put)}, forgot {sorted(gone)}")
    return any(k in GHOST_USES and k != "GITHUB_TOKEN" for k in values)


def add_password_routes(app: FastAPI, deps: Any) -> None:
    @app.get("/passwords", response_model=PasswordsView, tags=["passwords"])
    async def get_passwords() -> PasswordsView:
        return passwords_view(deps)

    @app.put("/passwords/{name}", response_model=PasswordResult, tags=["passwords"])
    async def put_password(name: str, body: PasswordSet) -> PasswordResult:
        """Save or replace one. The value is never returned, here or anywhere."""
        restart = save_passwords(deps, {name: body.value.get_secret_value()})
        await revive_apps_using(deps, [name])
        return PasswordResult(passwords=passwords_view(deps), restart_required=restart)

    @app.delete("/passwords/{name}", response_model=PasswordResult, tags=["passwords"])
    async def delete_password(name: str) -> PasswordResult:
        vault: Passwords | None = deps.passwords
        if vault is None or name not in vault:
            raise HTTPException(404, f"no password called {name}")
        restart = save_passwords(deps, {name: None})
        return PasswordResult(passwords=passwords_view(deps), restart_required=restart)

    @app.post("/passwords/unlock", response_model=PasswordsView, tags=["passwords"])
    async def unlock() -> PasswordsView:
        """Ask the keyring to unlock (it shows its own prompt on the desktop), then open
        the vault. The daemon follows: Ghost's own settings and the apps that waited."""
        vault: Passwords | None = deps.passwords
        if vault is None:
            raise HTTPException(503, "the vault is not available")
        if not vault.is_open and await asyncio.to_thread(vault.unlock):
            await deps.control.passwords_opened()
        return passwords_view(deps)

    @app.post("/passwords/import", response_model=PasswordImportResult, tags=["passwords"])
    async def import_all() -> PasswordImportResult:
        """Once: heka-vault and the plaintext env files into the vault, then the plaintext
        copies removed. A password already in the vault is kept."""
        vault: Passwords | None = deps.passwords
        if vault is None:
            raise HTTPException(503, "the vault is not available")
        try:
            report = await asyncio.to_thread(
                import_passwords,
                vault,
                deps.env_path.parent,
                heka_env=deps.vault_env_path,
                referenced=referenced(deps),
            )
        except VaultClosed as e:
            raise HTTPException(423, f"{e}. Unlock it, then try again.") from None
        await revive_apps_using(deps, report.imported)
        return PasswordImportResult(
            imported=report.imported,
            kept=report.kept,
            settings=report.settings,
            removed=[tilde(p) for p in report.removed],
            notes=report.notes,
            passwords=passwords_view(deps),
            restart_required=bool(report.imported or report.settings),
        )
