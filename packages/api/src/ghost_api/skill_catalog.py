"""Skills catalog routes (#35): search skills.sh, preview a skill from GitHub, install it
through the gate, and check installed skills for updates.

All of it is the operator's own doing, from the Skills page in Settings: the model has no
route here, and nothing here runs during a conversation. A search reaches skills.sh only
when the page asks (its results are cached an hour), a preview and an update check reach
GitHub only when the operator clicks. What the sources send is shown, never acted on.
"""

from __future__ import annotations

from datetime import UTC, datetime
from typing import Any

from fastapi import FastAPI, HTTPException, Query
from ghost_core.gate import GateError
from ghost_core.policy import PolicyError
from ghost_core.skill_catalog import Preview, SkillCatalogError, slug
from ghost_core.skills import GHOST_SOURCE, SOURCE, SkillOrigin

from ghost_api.app import Deps
from ghost_api.models import (
    SkillFileView,
    SkillInfo,
    SkillInstall,
    SkillListing,
    SkillOriginView,
    SkillPreview,
    SkillPreviewRequest,
    SkillSearchPage,
    SkillUpdateView,
)

UI_SESSION = "ui"  # the desktop's own session id for audited edits (as app.py)


def origin_view(o: SkillOrigin) -> SkillOriginView:
    return SkillOriginView(
        source=o.source,
        source_url=o.source_url,
        skill_path=o.skill_path,
        commit=o.commit,
        folder_hash=o.folder_hash,
        ref=o.ref,
        installed_at=o.installed_at or None,
    )


def skill_view(deps: Deps, skill: Any, origins: dict[str, SkillOrigin] | None = None) -> SkillInfo:
    """One skill for the desktop, with where it came from when Ghost installed it."""
    connected = (
        {n for n in deps.apps.apps if deps.apps.is_up(n)} if deps.apps is not None else set()
    )
    if origins is None:
        origins = deps.skills.origins() if deps.skills is not None else {}
    origin = origins.get(skill.name) if skill.source == GHOST_SOURCE else None
    return SkillInfo(
        name=skill.name,
        description=skill.description,
        source=skill.source,
        path=str(skill.path),
        requires=skill.apps(),
        missing=skill.missing(connected),
        files=skill.files(),
        problem=skill.problem,
        origin=origin_view(origin) if origin else None,
    )


def add_skill_catalog_routes(app: FastAPI, deps: Deps) -> None:
    def catalog() -> Any:
        if deps.skill_catalog is None or deps.skills is None:
            raise HTTPException(503, "the skills catalog is not available")
        return deps.skill_catalog

    def preview_view(p: Preview, update_of: str | None = None) -> SkillPreview:
        existing = next((s for s in deps.skills.all() if s.name == p.name), None)
        return SkillPreview(
            token=p.token,
            name=p.name,
            description=p.description,
            source=p.origin.source,
            source_url=p.origin.source_url,
            skill_path=p.origin.skill_path,
            commit=p.origin.commit,
            folder_hash=p.origin.folder_hash,
            ref=p.origin.ref,
            skill_md=p.skill_md,
            files=[SkillFileView(path=f.path, bytes=f.bytes, script=f.script) for f in p.files],
            scripts=p.scripts,
            installed=existing.source if existing is not None else None,  # type: ignore[arg-type]
            update_of=update_of,
        )

    @app.get("/skill-catalog", response_model=SkillSearchPage, tags=["skills"])
    async def search_skills(search: str = Query("", max_length=100)) -> SkillSearchPage:
        """Skills on skills.sh for a search of two letters or more, from Ghost's cache when
        recent. A failure is ``problem``, with the last results for the search if any."""
        res = await catalog().search(search)
        # A listing is installed when a skill came from its repository under its id: the
        # folder's name there, or the name Ghost installed it as.
        installed: dict[tuple[str, str], str] = {}
        for n, o in deps.skills.origins().items():
            installed[(o.source, slug(o.folder.rsplit("/", 1)[-1]))] = n
            installed.setdefault((o.source, n), n)
        return SkillSearchPage(
            query=res.query,
            skills=[
                SkillListing(
                    id=s.id,
                    source=s.source,
                    skill=s.skill,
                    name=s.name,
                    installs=s.installs,
                    installed=installed.get((s.source, slug(s.skill))),
                )
                for s in res.skills
            ],
            problem=res.problem,
            fetched_at=datetime.fromtimestamp(res.fetched_at, UTC) if res.fetched_at else None,
        )

    @app.post("/skill-catalog/preview", response_model=SkillPreview, tags=["skills"])
    async def preview_skill(req: SkillPreviewRequest) -> SkillPreview:
        """Fetch a skill's folder from GitHub at the commit its branch is on, and stage it:
        its SKILL.md and its files, for the operator to read before installing."""
        try:
            p = await catalog().preview(req.source, req.skill)
        except SkillCatalogError as e:
            raise HTTPException(409, str(e)) from None
        return preview_view(p)

    @app.post("/skill-catalog/install", response_model=SkillInfo, tags=["skills"])
    async def install_skill(req: SkillInstall) -> SkillInfo:
        """Copy a staged skill into Ghost's skills through the gate (``skills.install``),
        audited, with where it came from."""
        cat = catalog()
        try:
            p = cat.staged(req.token)
        except LookupError as e:
            raise HTTPException(404, str(e)) from None
        try:
            prepared = deps.skills.prepare_install(
                p.folder, p.origin, name=req.name, replace=req.replace
            )
        except ValueError as e:
            raise HTTPException(409, str(e)) from None
        try:
            out = await deps.gate.apply_internal(
                session_id=UI_SESSION,
                ability=SOURCE,
                operation="install",
                target=prepared.target,
                changes=prepared.changes,
                rationale=f"install {p.origin.source} from the desktop app",
            )
        except (GateError, PolicyError, ValueError) as e:
            raise HTTPException(409, str(e)) from None
        if out.status != "success":
            raise HTTPException(409, out.error or "not installed")
        cat.discard(req.token)
        deps.skills.refresh()
        name = prepared.target.name  # type: ignore[attr-defined]
        skill = next((s for s in deps.skills.all() if s.name == name), None)
        if skill is None:
            raise HTTPException(500, "the skill was copied but could not be read back")
        return skill_view(deps, skill)

    @app.get("/skill-catalog/updates", response_model=list[SkillUpdateView], tags=["skills"])
    async def skill_updates() -> list[SkillUpdateView]:
        """For each skill Ghost installed from GitHub, whether its folder has changed at
        the source since. Asks GitHub now (a recent answer per repository is reused)."""
        cat = catalog()
        origins = deps.skills.origins()
        found = await cat.check(origins)
        return [
            SkillUpdateView(
                name=name,
                source=origins[name].source,
                commit=origins[name].commit,
                latest_commit=latest.commit,
                update=latest.moved_on(origins[name]),
                problem=latest.problem,
            )
            for name, latest in found.items()
        ]

    @app.post("/skill-catalog/updates/{name}", response_model=SkillPreview, tags=["skills"])
    async def preview_update(name: str) -> SkillPreview:
        """The newer version of an installed skill, staged like any preview; installing it
        with ``replace`` and this name updates the skill."""
        origin = deps.skills.origins().get(name.strip().lower()) if deps.skills else None
        if origin is None:
            raise HTTPException(404, f"{name} wasn't installed from GitHub")
        try:
            p = await catalog().preview_update(origin)
        except SkillCatalogError as e:
            raise HTTPException(409, str(e)) from None
        return preview_view(p, update_of=name.strip().lower())
