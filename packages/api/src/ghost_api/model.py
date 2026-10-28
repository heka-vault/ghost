"""Model connection routes (ADR 0037): the providers, the connection now, its model list,
Test connection, and signing in to OpenRouter.

The connection itself is saved like any setting, through ``PUT /settings``: its
``GHOST_LLM_*`` values in settings.env. Its key is a password in Ghost's vault (ADR 0036),
saved on its own like any other (``PUT /passwords/{name}``). These routes read, list and
test, and keep the key OpenRouter's sign-in returns. A key goes in and never comes out:
views say whether the password is set, by name. The model has no route here.
"""

from __future__ import annotations

import asyncio
import os
from collections.abc import Mapping
from dataclasses import dataclass
from typing import Any

from fastapi import FastAPI, HTTPException
from ghost_core.connection import (
    ENV_KEYS,
    PROVIDERS,
    ModelConnection,
    format_headers,
    list_models,
    new_pkce,
    openrouter_auth_url,
    openrouter_exchange,
)
from ghost_core.envfile import read_env
from ghost_core.passwords import Passwords
from loguru import logger

from ghost_api.app import Deps
from ghost_api.models import (
    ModelConnectionView,
    ModelDraft,
    ModelListView,
    ModelOptionView,
    ModelProviderView,
    ModelSignInResult,
    ModelTestResult,
)
from ghost_api.passwords import save_passwords

SIGN_IN = "model:openrouter"  # the sign-in's name: no app can be called this
SIGN_IN_SECS = 600.0  # OpenRouter's codes last ten minutes
PROBE_PROMPT = "Say hello in five words."
PROBE_TOKENS = 32
# What a connection is read from: its own settings, and the older ones it falls back on.
CONNECTION_KEYS = (*ENV_KEYS, "GHOST_LLM_ENDPOINT", "ANTHROPIC_BASE_URL", "CLIPROXY_BASE_URL")
HELD = "(in the vault)"  # stands for a Password's value where only set-or-not is asked


@dataclass
class _SignIn:
    task: asyncio.Task[None] | None = None
    error: str | None = None

    @property
    def pending(self) -> bool:
        return self.task is not None and not self.task.done()


def model_env(deps: Deps, draft: ModelDraft | None = None, *, use: bool = False) -> dict[str, str]:
    """What a connection is resolved from: the daemon's environment (where Ghost's own
    passwords are, ADR 0036), the saved settings over it, then the draft. Its Password
    comes from the vault by name: its value only when ``use`` (a list or a test, noted as
    a use for "last used"); otherwise only whether it is there. A key typed into a draft
    is used for that one request and never kept."""
    env = dict(os.environ)
    saved = read_env(deps.settings_path)
    for k in CONNECTION_KEYS:
        if k in saved:
            env[k] = saved[k]
    if draft is not None:
        for k, v in draft.values.items():
            if k not in CONNECTION_KEYS:
                raise HTTPException(422, f"{k} is not a model connection setting")
            if v is None or v == "":
                env.pop(k, None)
            else:
                env[k] = v
    name = ModelConnection.from_env(env).password
    if not name:
        return env
    if draft is not None and draft.password_value:
        env[name] = draft.password_value.get_secret_value()
        return env
    vault: Passwords | None = deps.passwords
    if vault is not None and vault.is_open and name in vault:
        if use:
            env[name] = vault.value(name) or ""
        elif not env.get(name):
            env[name] = HELD  # set-or-not only; never sent anywhere
    return env


def explicit_values(conn: ModelConnection) -> dict[str, str]:
    """The GHOST_LLM_* values that say ``conn`` without the older settings."""
    out = {"GHOST_LLM_PROVIDER": conn.provider}
    if conn.model:
        out["GHOST_LLM_MODEL"] = conn.model
    if conn.provider == "custom":
        out["GHOST_LLM_PROTOCOL"] = conn.protocol
    if conn.base_url_override:
        out["GHOST_LLM_BASE_URL"] = conn.base_url_override
    if conn.password and conn.password != conn.spec.password:
        out["GHOST_LLM_PASSWORD"] = conn.password
    if conn.headers:
        out["GHOST_LLM_HEADERS"] = format_headers(conn.headers)
    if conn.timeout_secs:
        out["GHOST_LLM_TIMEOUT_SECS"] = f"{conn.timeout_secs:g}"
    return out


def providers_view() -> list[ModelProviderView]:
    return [
        ModelProviderView(
            id=p.id,
            title=p.title,
            blurb=p.blurb,
            protocol=p.protocol,
            base_url=p.base_url,
            password=p.password or None,
            password_optional=p.password_optional,
            sign_in=p.sign_in,
            local=p.local,
            default_model=p.default_model or None,
        )
        for p in PROVIDERS.values()
    ]


def add_model_routes(app: FastAPI, deps: Deps) -> None:
    sign_in = _SignIn()

    def locked() -> bool:
        """Ghost's passwords can't be read now: a key the environment lacks may be set."""
        vault: Passwords | None = deps.passwords
        return vault is not None and not vault.is_open

    def view(env: Mapping[str, str]) -> ModelConnectionView:
        conn = ModelConnection.from_env(env)
        problems = conn.problems(env, locked=locked())
        return ModelConnectionView(
            provider=conn.provider,
            protocol=conn.protocol,
            base_url=conn.base_url,
            base_url_override=conn.base_url_override or None,
            model=conn.model,
            wire_model=conn.wire_model,
            password=conn.password or None,
            password_set=conn.password_set(env),
            headers=dict(conn.headers),
            timeout_secs=conn.timeout_secs,
            legacy=conn.legacy or None,
            problems=problems,
            ready=not problems,
            values=explicit_values(conn),
            providers=providers_view(),
            sign_in_pending=sign_in.pending,
            sign_in_error=sign_in.error,
        )

    @app.get("/model", response_model=ModelConnectionView, tags=["model"])
    async def get_model() -> ModelConnectionView:
        """The model connection now, with every provider the page offers."""
        return view(model_env(deps))

    @app.post("/model/models", response_model=ModelListView, tags=["model"])
    async def model_list(draft: ModelDraft) -> ModelListView:
        """The models a provider offers, for the saved connection or the one being edited.
        A provider that can't be read says why; that is not an HTTP error."""
        env = model_env(deps, draft, use=True)
        conn = ModelConnection.from_env(env)
        default = conn.spec.default_model or None
        if not conn.base_url:
            return ModelListView(models=[], default=default, error="Give it an address first.")
        # OpenRouter lists its models to anyone; the others want the key, so don't ask
        # without one.
        if (
            conn.password
            and not conn.spec.password_optional
            and not conn.spec.sign_in
            and not conn.password_set(env)
        ):
            error = (
                "Unlock your keyring on Settings › Passwords to see the models it offers."
                if locked()
                else "Save its password to see the models it offers."
            )
            return ModelListView(models=[], default=default, error=error)
        try:
            found = await list_models(conn, env)
        except ValueError as e:
            return ModelListView(models=[], default=default, error=str(e))
        ids = {m.id for m in found}
        return ModelListView(
            models=[ModelOptionView(id=m.id, name=m.name or None) for m in found],
            default=default if default in ids else (found[0].id if found else default),
        )

    @app.post("/model/test", response_model=ModelTestResult, tags=["model"])
    async def model_test(draft: ModelDraft) -> ModelTestResult:
        """Send one tiny request through the connection being edited and time its first
        token, the way ``make perf-llm`` does."""
        if deps.model_probe is None:
            raise HTTPException(503, "Ghost can't test a model connection right now")
        env = model_env(deps, draft, use=True)
        conn = ModelConnection.from_env(env)
        where = conn.describe().split(" via ", 1)[-1]
        if conn.problems_in_settings:
            return ModelTestResult(
                ok=False, model=conn.wire_model, error=conn.problems_in_settings[0], where=where
            )
        probe: Any = await deps.model_probe(env, PROBE_PROMPT, PROBE_TOKENS)
        ms = lambda s: None if s is None else round(s * 1000)  # noqa: E731
        reply = (probe.text or "")[:120] or None
        return ModelTestResult(
            ok=probe.error is None and probe.ttft is not None,
            ttft_ms=ms(probe.ttft),
            total_ms=ms(probe.total),
            model=conn.wire_model,
            answered_by=probe.model or None,
            reply=reply,
            error=probe.error or (None if probe.ttft is not None else "No answer came back."),
            where=where,
        )

    @app.post("/model/sign-in", response_model=ModelSignInResult, tags=["model"])
    async def model_sign_in() -> ModelSignInResult:
        """Sign in to OpenRouter (OAuth with PKCE). The desktop opens the address; the
        browser comes back to Ghost's loopback, and the key OpenRouter gives is kept as
        the OPENROUTER_API_KEY password in Ghost's vault (ADR 0036). Nothing to copy."""
        if deps.signins is None:
            raise HTTPException(503, "Ghost can't take a sign-in right now")
        vault: Passwords | None = deps.passwords
        if vault is None:
            raise HTTPException(503, "the vault is not available")
        if not vault.is_open:
            # Asked first, so nobody signs in for a key Ghost then can't keep.
            raise HTTPException(423, "Ghost's passwords are locked. Unlock them, then sign in.")
        if sign_in.task is not None and not sign_in.task.done():
            sign_in.task.cancel()
        pkce = new_pkce()
        url = openrouter_auth_url(deps.signins.localhost_redirect_uri, pkce)
        sign_in.error = None

        async def finish() -> None:
            try:
                q = await deps.signins.browser_return(SIGN_IN, url, SIGN_IN_SECS)
                state = q.get("state")
                if state is not None and state != pkce.state:
                    raise ValueError("the sign-in came back for another request")
                if not q.get("code"):
                    raise ValueError("OpenRouter sent no code back")
                key = await openrouter_exchange(q["code"], pkce)
            except asyncio.CancelledError:
                raise
            except TimeoutError:
                sign_in.error = "Nobody finished signing in. Try again."
                return
            except Exception as e:  # noqa: BLE001 - said on the page, never the code or key
                sign_in.error = f"The sign-in didn’t go through: {e}"
                logger.info(f"model sign-in: {type(e).__name__}")
                return
            password = PROVIDERS["openrouter"].password
            try:
                # The vault, which puts it in the daemon's environment: the next session
                # uses it, no restart (ADR 0036).
                save_passwords(deps, {password: key})
            except HTTPException as e:
                sign_in.error = f"Ghost couldn’t keep the key: {e.detail}"
                return
            logger.info(f"model sign-in: OpenRouter's key kept as the {password} password")

        sign_in.task = asyncio.ensure_future(finish())
        await asyncio.sleep(0)  # starts listening; the browser comes back far later
        return ModelSignInResult(url=url)
