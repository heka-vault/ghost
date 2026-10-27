"""App routes (ADR 0034): what is connected, the catalog, connecting and disconnecting, the
sign-in, and the permissions.

Connecting is the operator's own action, from the desktop: it writes ``apps.json`` and puts
any secret into ``apps.env`` (0600, beside ``env``, which the vault renderer rewrites whole;
ghostd loads it at start) and into the running daemon's environment, so the app connects
at once. A secret value goes in and never comes back out; ``apps.json`` holds
only ``${NAME}``. The model has no route here.
"""

from __future__ import annotations

import asyncio
import dataclasses
import os
import re
import time
from datetime import UTC, datetime
from typing import Any
from urllib.parse import urlparse

from fastapi import FastAPI, HTTPException, Query
from ghost_core.apps import (
    APPS_FILE,
    RESERVED_NAMES,
    AppSpec,
    app_name_problem,
    extract_secrets,
    load_apps,
    parse_apps_json,
    secret_name,
    write_apps,
)
from ghost_core.catalog import CatalogEntry, CatalogError, connect
from ghost_core.levels import Autonomy
from ghost_core.permissions import Mode, Rule, parse_rules
from ghost_core.policy import AutonomyPolicy, write_permissions

from ghost_api.app import Deps, apps_env_path, permissions_view
from ghost_api.files import read_env, update_env
from ghost_api.models import (
    Ack,
    AppAction,
    AppConnect,
    AppConnectResult,
    AppView,
    CatalogEntryView,
    CatalogOption,
    CatalogPage,
    CatalogSetting,
    PermissionsUpdate,
    PermissionsView,
)

CONNECT_WAIT_SECS = 10.0  # how long POST /apps waits for a connection or a sign-in address
FIRST_COPY_WAIT_SECS = 15.0  # the catalog's first copy, so the screen has something to show
_LEVEL = {
    Autonomy.DENY: "block",
    Autonomy.CONFIRM: "ask",
    Autonomy.QUIET: "quiet",
    Autonomy.ANNOUNCE: "allow",
}


def add_app_routes(app: FastAPI, deps: Deps) -> None:  # noqa: C901 - one route table
    tasks: set[asyncio.Task[Any]] = set()  # connections under way, kept from the collector

    def apps_path():
        return deps.policy_path.parent / APPS_FILE

    def policy() -> AutonomyPolicy:
        return deps.gate.policy_for(None)

    def kind_of(spec: AppSpec) -> str:
        if spec.command:
            return "local"
        if spec.sign_in:
            return "sign_in"
        if spec.secrets():
            return "key"
        return "custom"

    def action_views(name: str) -> list[AppAction]:
        live = deps.apps.apps.get(name) if deps.apps is not None else None
        if live is None or not live.connected:
            return []
        perms = policy().permissions
        rules = [*perms.block, *perms.ask, *perms.quiet, *perms.allow]
        out = []
        for t in live.tools:
            conditional = any(r.conditional and r.names(name, t.name) for r in rules)
            if t.read_only:
                level = "block" if perms.blocks(name, t.name) else "allow"
            else:
                level = _LEVEL[perms.resolve(name, t.name)]
            out.append(
                AppAction(
                    name=t.name,
                    description=t.brief,
                    reads=t.read_only,
                    level=level,  # type: ignore[arg-type]
                    rule=f"{name}.{t.name}",
                    conditional=conditional,
                )
            )
        return out

    def app_view(name: str) -> AppView:
        spec = deps.apps.specs[name]
        entry = (
            deps.catalog.get(spec.catalog) if deps.catalog is not None and spec.catalog else None
        )
        url = deps.signins.pending_url(name) if deps.signins is not None else None
        return AppView(
            name=name,
            title=spec.shown,
            kind=kind_of(spec),  # type: ignore[arg-type]
            state=deps.apps.state(name),  # type: ignore[arg-type]
            problem=deps.apps.problem(name),
            catalog=spec.catalog,
            icon=entry.icon if entry is not None else None,
            actions=action_views(name),
            sign_in_url=url,
            settings=spec.secrets(),
        )

    def need_apps() -> None:
        if deps.apps is None:
            raise HTTPException(503, "apps are not available")

    @app.get("/apps", response_model=list[AppView], tags=["apps"])
    async def list_apps() -> list[AppView]:
        """Every app in apps.json: connected or not, and what each tool may do."""
        need_apps()
        return [app_view(n) for n in deps.apps.specs]

    @app.get("/apps/{name}", response_model=AppView, tags=["apps"])
    async def get_app(name: str) -> AppView:
        need_apps()
        if name not in deps.apps.specs:
            raise HTTPException(404, f"no app called {name!r}")
        return app_view(name)

    # --- the catalog ---------------------------------------------------------------------

    def entry_view(e: CatalogEntry) -> CatalogEntryView:
        connected = (
            [n for n, s in deps.apps.specs.items() if s.catalog == e.name]
            if deps.apps is not None
            else []
        )
        return CatalogEntryView(
            name=e.name,
            title=e.title,
            description=e.description,
            version=e.version,
            website=e.website,
            repository=e.repository,
            icon=e.icon,
            featured=e.featured,
            options=[
                CatalogOption(
                    id=o.id,
                    kind=o.kind,
                    label=o.label,
                    settings=[
                        CatalogSetting(
                            key=s.key,
                            label=s.label,
                            secret=s.secret,
                            required=s.required,
                            default=s.default,
                            choices=list(s.choices),
                        )
                        for s in o.settings
                    ],
                )
                for o in e.options
            ],
            suggested_name=free_name(e.app_name),
            connected=connected,
        )

    def free_name(base: str) -> str:
        """``base``, or ``base-2`` and on when an app has it or Ghost's own tools do."""
        taken = set(deps.apps.specs) if deps.apps is not None else set()
        taken |= RESERVED_NAMES
        if base not in taken:
            return base
        n = 2
        while f"{base}-{n}" in taken:
            n += 1
        return f"{base}-{n}"

    @app.get("/catalog", response_model=CatalogPage, tags=["apps"])
    async def catalog(
        search: str = Query("", max_length=100),
        offset: int = Query(0, ge=0),
        limit: int = Query(40, ge=1, le=100),
    ) -> CatalogPage:
        """Apps to connect, from Ghost's copy of the registry; first-party ones first when
        nothing is searched. An old copy is refreshed in the background."""
        cat = deps.catalog
        if cat is None:
            raise HTTPException(503, "the catalog is not available")
        cat.refresh_soon()
        if not cat.fetched_at:
            await cat.wait(FIRST_COPY_WAIT_SECS)
        entries, total = cat.search(search, limit=limit, offset=offset)
        return CatalogPage(
            entries=[entry_view(e) for e in entries],
            total=total,
            fetched_at=datetime.fromtimestamp(cat.fetched_at, UTC) if cat.fetched_at else None,
            refreshing=cat.refreshing,
            problem=cat.problem,
        )

    @app.get("/catalog/{name:path}", response_model=CatalogEntryView, tags=["apps"])
    async def catalog_entry(name: str) -> CatalogEntryView:
        e = deps.catalog.get(name) if deps.catalog is not None else None
        if e is None:
            raise HTTPException(404, f"{name} is not in the catalog")
        return entry_view(e)

    # --- connecting ----------------------------------------------------------------------

    def keep_secrets(secrets: dict[str, str]) -> None:
        """Into apps.env and the running daemon's environment; never logged."""
        if not secrets:
            return
        update_env(apps_env_path(deps), dict(secrets), mode=0o600)
        os.environ.update(secrets)

    def forget_secrets(spec: AppSpec) -> None:
        """The keys Ghost kept for this app: the ``GHOST_APP_`` names its own entry refers
        to and no other app's does. A name the operator chose themselves stays, and so
        does every other app's key (``github-2``'s are not ``github``'s)."""
        others = {n for s in deps.apps.specs.values() if s.name != spec.name for n in s.secrets()}
        path = apps_env_path(deps)
        kept = read_env(path)
        gone = [k for k in spec.secrets() if k.startswith("GHOST_APP_") and k not in others]
        gone = [k for k in gone if k in kept]
        if gone:
            update_env(path, dict.fromkeys(gone), mode=0o600)
            for k in gone:
                os.environ.pop(k, None)

    def clash(spec: AppSpec, secrets: dict[str, str]) -> str | None:
        """Another app whose keys this one's would overwrite: ``my-app`` and ``my_app``
        keep theirs under the same names."""
        prefix = secret_name(spec.name, "")
        for other in deps.apps.specs.values():
            if other.name == spec.name:
                continue
            if secret_name(other.name, "") == prefix or set(secrets) & set(other.secrets()):
                return other.name
        return None

    def save(specs: list[AppSpec], *, remove: str | None = None) -> None:
        current = dict(load_apps(apps_path()).apps)
        if remove is not None:
            current.pop(remove, None)
        for spec in specs:
            current[spec.name] = spec
        write_apps(apps_path(), current)

    async def start(specs: list[AppSpec]) -> str | None:
        """Connect each now, in the background; wait a little for a connection or for a
        sign-in to want the browser. Returns the sign-in address, if one does."""
        waiting = []
        for spec in specs:
            task = asyncio.ensure_future(deps.apps.add(spec, interactive=True))
            tasks.add(task)
            task.add_done_callback(
                lambda t, n=spec.name: (tasks.discard(t), deps.state.publish_apps(changed=n))
            )
            waiting.append((spec, task))
        deadline = time.monotonic() + CONNECT_WAIT_SECS
        while time.monotonic() < deadline:
            for spec, _task in waiting:
                url = deps.signins.pending_url(spec.name) if deps.signins is not None else None
                if url:
                    return url
            if all(t.done() for _, t in waiting):
                return None
            await asyncio.sleep(0.1)
        return None

    def specs_from(req: AppConnect) -> tuple[list[AppSpec], dict[str, str]]:
        values = req.values or {}
        if req.config is not None:
            try:
                parsed = parse_apps_json(req.config, "the pasted config")
            except ValueError as e:
                raise HTTPException(422, str(e)) from None
            if not parsed.apps:
                raise HTTPException(422, "; ".join(parsed.problems) or "no apps in it")
            specs, secrets = [], {}
            for spec in parsed.apps.values():
                moved, found = extract_secrets(spec)
                specs.append(moved)
                secrets.update(found)
            return specs, secrets
        if req.url is not None:
            u = urlparse(req.url)
            if u.scheme not in ("https", "http") or not u.netloc:
                raise HTTPException(422, "the address must start with https://")
            name = req.name or re.sub(r"[^a-z0-9]+", "-", u.netloc.split(":")[0].lower())
            name = name.removeprefix("mcp-").removesuffix("-com").strip("-")[:40] or "app"
            headers: dict[str, str] = {}
            secrets: dict[str, str] = {}
            key = (values.get("key") or "").strip()
            if key:
                env = secret_name(name, "key")
                secrets[env] = key if " " in key else f"Bearer {key}"
                headers["Authorization"] = "${" + env + "}"
            spec = AppSpec(
                name=name,
                title=values.get("title", ""),
                url=req.url,
                headers=headers,
                sign_in=not key,
            )
            return [spec], secrets
        if req.catalog is None or req.option is None:
            raise HTTPException(422, "give catalog and option, a url, or a config")
        entry = deps.catalog.get(req.catalog) if deps.catalog is not None else None
        if entry is None:
            raise HTTPException(404, f"{req.catalog} is not in the catalog")
        try:
            spec, secrets = connect(
                entry, req.option, req.name or free_name(entry.app_name), values
            )
        except CatalogError as e:
            raise HTTPException(422, str(e)) from None
        return [spec], secrets

    @app.post("/apps", response_model=AppConnectResult, tags=["apps"])
    async def connect_app(req: AppConnect) -> AppConnectResult:
        """Connect an app now and keep it in apps.json. An app that signs in comes back
        with the address the browser should open; the desktop opens it."""
        need_apps()
        specs, secrets = specs_from(req)
        for spec in specs:
            if why := app_name_problem(spec.name):
                raise HTTPException(422, f"{spec.name!r}: {why}")
            if spec.name in deps.apps.specs and not req.replace:
                raise HTTPException(409, f"an app is already connected as {spec.name}")
            if other := clash(spec, secrets):
                raise HTTPException(
                    409, f"{spec.name} would share its keys with {other}; give it another name"
                )
        keep_secrets(secrets)
        save(specs)
        url = await start(specs)
        return AppConnectResult(apps=[app_view(s.name) for s in specs], sign_in_url=url)

    @app.post("/apps/{name}/sign-in", response_model=AppConnectResult, tags=["apps"])
    async def sign_in(name: str) -> AppConnectResult:
        """Sign in again, or reconnect an app that isn't connected."""
        need_apps()
        spec = deps.apps.specs.get(name)
        if spec is None:
            raise HTTPException(404, f"no app called {name!r}")
        if deps.signins is not None and spec.sign_in:
            deps.signins.forget(name)  # a fresh registration and tokens
        url = await start([spec])
        return AppConnectResult(apps=[app_view(name)], sign_in_url=url)

    @app.delete("/apps/{name}", response_model=Ack, tags=["apps"])
    async def disconnect(name: str) -> Ack:
        """Disconnect: it leaves the tools at once and apps.json; its sign-in and the keys
        Ghost kept for it are forgotten."""
        need_apps()
        spec = deps.apps.specs.get(name)
        if spec is None:
            raise HTTPException(404, f"no app called {name!r}")
        await deps.apps.remove(name)
        if deps.signins is not None:
            deps.signins.forget(name)
        save([], remove=name)
        forget_secrets(spec)
        deps.state.publish_apps(changed=name)
        return Ack(ok=True, detail=None)

    # --- permissions ---------------------------------------------------------------------

    @app.get("/permissions", response_model=PermissionsView, tags=["permissions"])
    async def get_permissions() -> PermissionsView:
        text = deps.policy_path.read_text() if deps.policy_path.exists() else ""
        try:
            p = AutonomyPolicy.parse(text, source=str(deps.policy_path))
        except ValueError as e:
            raise HTTPException(422, str(e)) from None
        return permissions_view(p, deps.policy_path)

    @app.put("/permissions", response_model=PermissionsView, tags=["permissions"])
    async def put_permissions(update: PermissionsUpdate) -> PermissionsView:
        """Change the mode, whole lists, or one rule; only ``[permissions]`` in the file is
        rewritten, and the gate follows at once."""
        text = deps.policy_path.read_text() if deps.policy_path.exists() else ""
        try:
            perms = AutonomyPolicy.parse(text, source=str(deps.policy_path)).permissions
            changes: dict[str, Any] = {}
            if update.mode is not None:
                changes["mode"] = Mode(update.mode)
            for key in ("block", "ask", "quiet", "allow"):
                value = getattr(update, key)
                if value is not None:
                    changes[key] = parse_rules(value, key)
            if changes:
                perms = dataclasses.replace(perms, **changes)
            if update.set is not None:
                perms = perms.with_rule(Rule.parse(update.set.rule), update.set.level)
            out = write_permissions(text, perms)
        except ValueError as e:
            raise HTTPException(422, str(e)) from None
        deps.policy_path.parent.mkdir(parents=True, exist_ok=True)
        deps.policy_path.write_text(out)
        p = deps.control.reload_policy()
        deps.state.publish_apps()
        return permissions_view(p, deps.policy_path)
