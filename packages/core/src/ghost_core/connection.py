"""The model connection (ADR 0037): one place that says where every model call goes.

Spoken sessions with their tool calls, compaction, a watch's model step and the latency
harness all reach the model through the connection this module resolves. A provider is
picked like an app is: Anthropic with a password, OpenRouter by signing in, the operator's
Claude subscription through CLIProxy, OpenAI or a local Ollama or LM Studio, or a URL of
the operator's own that speaks either protocol.

Two protocols carry it: Anthropic's Messages API and the OpenAI-compatible Chat
Completions API (``ghost_voice.llm`` has a client for each). The key is a Password: the
connection names it (``GHOST_LLM_PASSWORD``, a variable in Ghost's environment) and it is
read by that name when a target is built, never stored here, shown, or logged.

The non-secret settings, all in settings.env:

    GHOST_LLM_PROVIDER     anthropic, openrouter, cliproxy, openai, ollama, lmstudio, custom
    GHOST_LLM_MODEL        the model id as the provider names it (a default per provider)
    GHOST_LLM_PROTOCOL     anthropic or openai; a custom URL only (the others are fixed)
    GHOST_LLM_BASE_URL     overrides the provider's address (folded away in the desktop)
    GHOST_LLM_PASSWORD     the Password's name, when not the provider's own
    GHOST_LLM_HEADERS      extra headers, ``Name: value; Name: value``, sent as written
    GHOST_LLM_TIMEOUT_SECS how long a call may take before it fails

With no ``GHOST_LLM_PROVIDER`` the settings from before keep working unchanged:
``GHOST_LLM_ENDPOINT=openrouter`` is OpenRouter with ``OPENROUTER_API_KEY``, and the
default (``proxy``) is ``ANTHROPIC_BASE_URL`` with ``ANTHROPIC_AUTH_TOKEN``.
"""

from __future__ import annotations

import base64
import hashlib
import json
import re
import secrets
from collections.abc import Mapping
from dataclasses import dataclass, field
from typing import Any, Literal
from urllib.parse import urlencode, urlparse

import httpx

Protocol = Literal["anthropic", "openai"]
PROTOCOLS: tuple[str, ...] = ("anthropic", "openai")
LEGACY_ENDPOINTS: tuple[str, ...] = ("proxy", "openrouter")
DEFAULT_CLAUDE = "claude-sonnet-5-5"
ANTHROPIC_VERSION = "2023-06-01"
OPENROUTER_ANTHROPIC_URL = "https://openrouter.ai/api"  # OpenRouter's Anthropic Messages API
OPENROUTER_AUTH_URL = "https://openrouter.ai/auth"
OPENROUTER_KEYS_URL = "https://openrouter.ai/api/v1/auth/keys"
CLIPROXY_DEFAULT_URL = "http://localhost:8317"
LIST_TIMEOUT_SECS = 8.0

# The environment keys this module reads (besides the Password it names).
ENV_KEYS = (
    "GHOST_LLM_PROVIDER",
    "GHOST_LLM_MODEL",
    "GHOST_LLM_PROTOCOL",
    "GHOST_LLM_BASE_URL",
    "GHOST_LLM_PASSWORD",
    "GHOST_LLM_HEADERS",
    "GHOST_LLM_TIMEOUT_SECS",
)
_NAME = re.compile(r"^[A-Z][A-Z0-9_]{1,63}$")
_CLAUDE_ID = re.compile(r"^(claude-[a-z]+)-(\d+)(?:-(\d{1,2}))?$")


@dataclass(frozen=True)
class Provider:
    """One way to reach a model, as the desktop offers it."""

    id: str
    title: str
    blurb: str
    protocol: Protocol
    base_url: str
    password: str = ""  # the Password's name; "" when none is needed
    password_optional: bool = False  # a local server, or a proxy that may take no key
    sign_in: bool = False  # OpenRouter: the key comes back from signing in
    local: bool = False  # runs on this machine (or the operator's network)
    default_model: str = ""  # "" means the first the provider lists
    # The Chat Completions field for the output cap: OpenAI's own API wants
    # max_completion_tokens; the compatible servers know max_tokens.
    cap_param: str = "max_tokens"


PROVIDERS: dict[str, Provider] = {
    p.id: p
    for p in (
        Provider(
            id="anthropic",
            title="Anthropic",
            blurb="Claude, with a password (API key) from the Anthropic Console.",
            protocol="anthropic",
            base_url="https://api.anthropic.com",
            password="ANTHROPIC_API_KEY",
            default_model=DEFAULT_CLAUDE,
        ),
        Provider(
            id="openrouter",
            title="OpenRouter",
            blurb="Sign in, and Claude or any other model OpenRouter offers.",
            protocol="anthropic",
            base_url=OPENROUTER_ANTHROPIC_URL,
            password="OPENROUTER_API_KEY",
            sign_in=True,
            default_model="anthropic/claude-sonnet-5.5",
        ),
        Provider(
            id="cliproxy",
            title="Claude subscription",
            blurb="Your Claude plan, through CLIProxy running on your network.",
            protocol="anthropic",
            base_url=CLIPROXY_DEFAULT_URL,
            password="ANTHROPIC_AUTH_TOKEN",
            password_optional=True,
            local=True,
            default_model=DEFAULT_CLAUDE,
        ),
        Provider(
            id="openai",
            title="OpenAI",
            blurb="GPT models, with a password (API key) from the OpenAI platform.",
            protocol="openai",
            base_url="https://api.openai.com/v1",
            password="OPENAI_API_KEY",
            default_model="gpt-5",
            cap_param="max_completion_tokens",
        ),
        Provider(
            id="ollama",
            title="Ollama",
            blurb="A model running on this computer. Nothing leaves it.",
            protocol="openai",
            base_url="http://localhost:11434/v1",
            local=True,
        ),
        Provider(
            id="lmstudio",
            title="LM Studio",
            blurb="A model LM Studio serves on this computer. Nothing leaves it.",
            protocol="openai",
            base_url="http://localhost:1234/v1",
            local=True,
        ),
        Provider(
            id="custom",
            title="Your own address",
            blurb="Any address that speaks Anthropic’s or OpenAI’s API.",
            protocol="anthropic",
            base_url="",
            password="LLM_API_KEY",
            password_optional=True,
        ),
    )
}

# Every Password a provider may use by default: the desktop may set these.
PASSWORD_NAMES: frozenset[str] = frozenset(p.password for p in PROVIDERS.values() if p.password)


def openrouter_model(model: str) -> str:
    """A Claude id as OpenRouter names it: ``claude-sonnet-5-5`` is
    ``anthropic/claude-sonnet-5.5``. An id that already has a vendor prefix is kept."""
    if "/" in model:
        return model
    m = _CLAUDE_ID.match(model)
    if not m:
        return f"anthropic/{model}"
    name, major, minor = m.groups()
    return f"anthropic/{name}-{major}" + (f".{minor}" if minor else "")


def is_password_name(name: str) -> bool:
    return bool(_NAME.match(name))


def password_names(values: Mapping[str, str | None]) -> set[str]:
    """The Passwords a model connection may use: every provider's own, and the one the
    settings name (``GHOST_LLM_PASSWORD``)."""
    names = set(PASSWORD_NAMES)
    named = (values.get("GHOST_LLM_PASSWORD") or "").strip()
    if is_password_name(named):
        names.add(named)
    return names


_HEADER_NAME = re.compile(r"^[A-Za-z0-9!#%&'*+.^_`|~-]+$")
# Kept out of a value: what settings.env would have to quote or escape, and what a shell
# reading the file would expand. A header that carries a key belongs in the Password.
_HEADER_UNSAFE = set('"\\$;\n\r')


def parse_headers(raw: str | None) -> dict[str, str]:
    """``GHOST_LLM_HEADERS``: ``Name: value`` pairs separated by semicolons. Raises
    ValueError with a sentence the operator can act on."""
    out: dict[str, str] = {}
    for part in (raw or "").split(";"):
        if not part.strip():
            continue
        name, sep, value = part.partition(":")
        name, value = name.strip(), value.strip()
        if not sep or not name:
            raise ValueError("write extra headers as Name: value, separated by semicolons")
        if not _HEADER_NAME.match(name):
            raise ValueError(f"{name!r} is not a header name")
        if _HEADER_UNSAFE & set(value):
            raise ValueError(f"{name}'s value can't hold quotes, backslashes, $ or ;")
        out[name] = value
    return out


def format_headers(headers: Mapping[str, str]) -> str:
    """The inverse of ``parse_headers``, checked the same way."""
    text = "; ".join(f"{k}: {v}" for k, v in headers.items())
    parse_headers(text)
    return text


# --- models that reason ---------------------------------------------------------------

# Room for a reasoning model's hidden tokens on top of the reply's own cap. OpenAI counts
# reasoning against max_completion_tokens, so without it a tool-heavy turn could end at
# finish_reason=length before a word or a call came out. A cap only bounds a reply: an
# answer that needs less costs nothing more.
REASONING_HEADROOM = 6000

_OPENAI_O = re.compile(r"^o(\d+)(-[a-z0-9.-]+)?$")
_OPENAI_GPT = re.compile(r"^gpt-(\d+)(?:\.(\d+))?(-[a-z0-9.-]+)?$")


@dataclass(frozen=True)
class Reasoning:
    """What a model does before it answers, as far as Ghost's request needs to know."""

    reasons: bool = False  # spends output tokens on reasoning before the reply
    effort: str = ""  # the reasoning_effort to send; "" sends none


def reasoning(model: str) -> Reasoning:
    """OpenAI's reasoning models, by id (``openai/`` prefix allowed), and the lowest
    reasoning effort each takes on Chat Completions with tools, since Ghost answers by
    voice and every token of thought is a pause (OpenAI's model docs, 2026-10):

    - GPT-5.1 and later, and GPT-6: ``none``, which is also the only effort GPT-5.4 and
      later combine with function tools on Chat Completions. GPT-6 Astra and GPT-6.1 Sol
      don't take ``none``: ``low``.
    - The first GPT-5 models (gpt-5, -mini, -nano): ``low``. Their ``minimal`` exists but
      turns off parallel tool calls, and Ghost runs every call of a reply together.
    - o-series: ``low``. o1-mini and o1-preview take no effort at all.
    - A ``-pro`` model only reasons at ``high`` and a ``-chat`` one is tuned for chat:
      neither gets an effort, both get the headroom.

    Anything else (Claude, a local model, an id this doesn't know) is not a reasoning model
    here, and its request is as before."""
    mid = model.strip().lower().removeprefix("openai/")
    if _OPENAI_O.match(mid):
        if mid.startswith(("o1-mini", "o1-preview")):
            return Reasoning(reasons=True)
        return Reasoning(reasons=True, effort="low")
    m = _OPENAI_GPT.match(mid)
    if not m or int(m.group(1)) < 5:
        return Reasoning()
    major, minor, rest = int(m.group(1)), m.group(2), m.group(3) or ""
    if "-chat" in rest or "-pro" in rest:
        return Reasoning(reasons=True)
    if major == 5 and minor is None:
        return Reasoning(reasons=True, effort="low")
    if (major, minor) == (6, None) and rest.startswith("-astra"):
        return Reasoning(reasons=True, effort="low")
    if (major, minor) == (6, "1") and rest.startswith("-sol"):
        return Reasoning(reasons=True, effort="low")
    return Reasoning(reasons=True, effort="none")


def _host(url: str) -> str:
    u = urlparse(url)
    return f"{u.scheme}://{u.hostname}" + (f":{u.port}" if u.port else "") if u.hostname else ""


@dataclass(frozen=True)
class Target:
    """A connection resolved for one call site: where, which model, the key. ``api_key``
    and resolved ``headers`` are never in a repr, and ``describe`` leaves them out."""

    provider: str
    protocol: Protocol
    base_url: str
    model: str  # as the provider names it on the wire
    api_key: str = field(default="", repr=False)
    headers: Mapping[str, str] = field(default_factory=dict, repr=False)
    timeout_secs: float | None = None
    cap_param: str = "max_tokens"
    reasoning: Reasoning = field(default_factory=Reasoning)  # Chat Completions only

    @property
    def endpoint(self) -> str:  # the older name, for logs and reports
        return self.provider

    def output_cap(self, max_tokens: int) -> int:
        """The cap to send for a reply of up to ``max_tokens`` visible tokens: more for a
        model that spends some of it reasoning first."""
        return max_tokens + REASONING_HEADROOM if self.reasoning.reasons else max_tokens

    def chat_params(self, max_tokens: int) -> dict[str, Any]:
        """The Chat Completions fields for the cap and the reasoning effort."""
        out: dict[str, Any] = {self.cap_param: self.output_cap(max_tokens)}
        if self.reasoning.effort:
            out["reasoning_effort"] = self.reasoning.effort
        return out

    def describe(self) -> str:
        where = _host(self.base_url) or "no address"
        return f"{self.model} via {self.provider} ({self.protocol}, {where})"

    def fingerprint(self) -> str:
        """What tells two targets' clients apart, without holding the key."""
        h = hashlib.sha256()
        for part in (
            self.protocol,
            self.base_url,
            self.api_key,
            json.dumps(dict(self.headers), sort_keys=True),
            str(self.timeout_secs),
        ):
            h.update(part.encode())
            h.update(b"\0")
        return h.hexdigest()


@dataclass(frozen=True)
class ModelConnection:
    """The operator's model connection, before any Password is read."""

    provider: str
    protocol: Protocol
    base_url: str  # resolved: the override, else the provider's
    model: str  # as the operator wrote it ("" means the provider's default)
    password: str = ""  # a name
    headers: Mapping[str, str] = field(default_factory=dict)  # sent as written
    timeout_secs: float | None = None
    base_url_override: str = ""
    legacy: str = ""  # the older setting this came from, when it did
    problems_in_settings: tuple[str, ...] = ()

    @property
    def spec(self) -> Provider:
        return PROVIDERS[self.provider]

    @property
    def wire_model(self) -> str:
        model = self.model or self.spec.default_model
        if not model and self.protocol == "anthropic":
            model = DEFAULT_CLAUDE  # an Anthropic-compatible URL: Claude, as before
        if self.provider == "openrouter" and model:
            return openrouter_model(model)
        return model

    def key(self, env: Mapping[str, str]) -> str:
        return env.get(self.password, "") if self.password else ""

    def target(self, env: Mapping[str, str]) -> Target:
        model = self.wire_model
        cap_param, thinks = "max_tokens", Reasoning()
        if self.protocol == "openai":
            # A reasoning model is one wherever it is served, and OpenAI's take only
            # max_completion_tokens, so the model decides both, not the provider.
            thinks = reasoning(model)
            cap_param = "max_completion_tokens" if thinks.reasons else self.spec.cap_param
        return Target(
            provider=self.provider,
            protocol=self.protocol,
            base_url=self.base_url,
            model=model,
            api_key=self.key(env),
            headers=dict(self.headers),
            timeout_secs=self.timeout_secs,
            cap_param=cap_param,
            reasoning=thinks,
        )

    def password_set(self, env: Mapping[str, str]) -> bool:
        return bool(self.key(env))

    def problems(self, env: Mapping[str, str], *, locked: bool = False) -> list[str]:
        """What stops the model from being reachable, in words, without naming a value.
        ``locked``: Ghost's passwords can't be read now (the keyring is locked), so a key
        missing from ``env`` may well be set; that is said instead of "not set"."""
        out = list(self.problems_in_settings)
        if not self.base_url:
            out.append(f"{self.spec.title} needs an address (GHOST_LLM_BASE_URL)")
        if not self.wire_model:
            out.append("no model is chosen (GHOST_LLM_MODEL)")
        needs_key = self.password and not self.spec.password_optional
        if needs_key and not self.password_set(env):
            if locked:
                out.append(
                    f"your keyring is locked, so Ghost can't read {self.password}."
                    " Unlock it on Settings › Passwords"
                )
            elif self.legacy:
                out.append(f"{self.legacy} but {self.password} is not set")
            elif self.spec.sign_in:
                out.append(f"sign in to {self.spec.title} ({self.password} is not set)")
            else:
                out.append(f"{self.password} is not set")
        return out

    def describe(self) -> str:
        where = _host(self.base_url) or "no address"
        return f"{self.wire_model or '(no model)'} via {self.provider} ({self.protocol}, {where})"

    @classmethod
    def from_env(cls, env: Mapping[str, str]) -> ModelConnection:
        """The connection the environment describes. Unknown values don't raise: they are
        problems, so a typo shows on the desktop instead of keeping Ghost from starting."""
        problems: list[str] = []
        get = lambda k: (env.get(k) or "").strip()  # noqa: E731
        provider = get("GHOST_LLM_PROVIDER").lower()
        legacy = ""
        base_override = get("GHOST_LLM_BASE_URL")
        password = get("GHOST_LLM_PASSWORD")
        protocol = get("GHOST_LLM_PROTOCOL").lower()
        if provider and provider not in PROVIDERS:
            problems.append(f"GHOST_LLM_PROVIDER={provider} is not one of {', '.join(PROVIDERS)}")
            provider = ""
        if not provider:
            # Before the connection: GHOST_LLM_ENDPOINT picked proxy or OpenRouter.
            endpoint = get("GHOST_LLM_ENDPOINT").lower() or "proxy"
            if endpoint == "openrouter":
                provider, legacy = "openrouter", "GHOST_LLM_ENDPOINT=openrouter"
            else:
                legacy_base = get("ANTHROPIC_BASE_URL")
                cliproxy = get("CLIPROXY_BASE_URL")
                if legacy_base and cliproxy and legacy_base.rstrip("/") == cliproxy.rstrip("/"):
                    provider = "cliproxy"
                elif legacy_base:
                    provider = "custom"
                else:
                    provider = "anthropic"
                base_override = base_override or legacy_base
                if not password and (provider != "anthropic" or env.get("ANTHROPIC_AUTH_TOKEN")):
                    password = "ANTHROPIC_AUTH_TOKEN"
        spec = PROVIDERS[provider]
        if provider == "custom":
            if protocol and protocol not in PROTOCOLS:
                problems.append(f"GHOST_LLM_PROTOCOL={protocol} is not anthropic or openai")
                protocol = ""
            resolved_protocol = protocol or "anthropic"
        else:
            resolved_protocol = spec.protocol
        base = base_override
        if not base and provider == "cliproxy":
            base = get("CLIPROXY_BASE_URL") or spec.base_url
        base = (base or spec.base_url).rstrip("/")
        if base_override and not re.match(r"^https?://\S+$", base_override):
            problems.append("GHOST_LLM_BASE_URL must be a full URL, like http://localhost:8080")
        if password and not is_password_name(password):
            problems.append("GHOST_LLM_PASSWORD must be a name like MY_MODEL_KEY")
            password = ""
        try:
            headers = parse_headers(env.get("GHOST_LLM_HEADERS"))
        except ValueError as e:
            problems.append(f"GHOST_LLM_HEADERS: {e}")
            headers = {}
        timeout: float | None = None
        if raw := get("GHOST_LLM_TIMEOUT_SECS"):
            try:
                timeout = float(raw)
                if timeout <= 0:
                    raise ValueError
            except ValueError:
                problems.append("GHOST_LLM_TIMEOUT_SECS must be a number of seconds above 0")
                timeout = None
        return cls(
            provider=provider,
            protocol=resolved_protocol,  # type: ignore[arg-type]
            base_url=base,
            model=get("GHOST_LLM_MODEL"),
            password=password or spec.password,
            headers=headers,
            timeout_secs=timeout,
            base_url_override=base_override,
            legacy=legacy,
            problems_in_settings=tuple(problems),
        )


# --- the model list -------------------------------------------------------------------


@dataclass(frozen=True)
class ModelOption:
    id: str
    name: str = ""


def _parse_models(data: Any) -> list[ModelOption]:
    rows = data.get("data") if isinstance(data, dict) else data
    if not isinstance(rows, list):
        # Ollama's own /api/tags shape, should a base URL point there.
        rows = data.get("models") if isinstance(data, dict) else None
        if not isinstance(rows, list):
            raise ValueError("the answer had no model list in it")
    out: dict[str, ModelOption] = {}
    for r in rows:
        if not isinstance(r, dict):
            continue
        mid = r.get("id") or r.get("name") or r.get("model")
        if not isinstance(mid, str) or not mid:
            continue
        name = r.get("display_name") or r.get("name") or ""
        out[mid] = ModelOption(id=mid, name=name if isinstance(name, str) and name != mid else "")
    return sorted(out.values(), key=lambda m: m.id)


def models_url(conn: ModelConnection) -> str:
    """Where the provider lists its models: ``/v1/models`` beside the Messages API, or
    ``/models`` under an OpenAI-compatible base (which already ends in ``/v1``)."""
    return conn.base_url + ("/v1/models" if conn.protocol == "anthropic" else "/models")


async def list_models(
    conn: ModelConnection,
    env: Mapping[str, str],
    *,
    client: httpx.AsyncClient | None = None,
) -> list[ModelOption]:
    """The provider's models. Raises ``ValueError`` with a short reason; the key is sent
    only to the provider's own address and never appears in the reason."""
    headers = dict(conn.headers)
    key = conn.key(env)
    if conn.protocol == "anthropic":
        headers["anthropic-version"] = ANTHROPIC_VERSION
        if key:
            headers["x-api-key"] = key
            if conn.provider != "anthropic":
                headers["Authorization"] = f"Bearer {key}"  # OpenRouter, most proxies
    elif key:
        headers["Authorization"] = f"Bearer {key}"
    own = client is None
    http = client or httpx.AsyncClient(timeout=LIST_TIMEOUT_SECS)
    try:
        params = {"limit": "1000"} if conn.provider == "anthropic" else None
        r = await http.get(models_url(conn), headers=headers, params=params)
    except httpx.HTTPError as e:
        raise ValueError(
            f"couldn’t reach {_host(conn.base_url) or 'it'} ({type(e).__name__})"
        ) from None
    finally:
        if own:
            await http.aclose()
    if r.status_code in (401, 403):
        raise ValueError(f"{conn.spec.title} refused the key (HTTP {r.status_code})")
    if r.status_code >= 400:
        raise ValueError(f"{_host(conn.base_url)} answered HTTP {r.status_code}")
    try:
        return _parse_models(r.json())
    except ValueError as e:
        raise ValueError(str(e) if "model list" in str(e) else "the answer wasn’t JSON") from None


# --- signing in to OpenRouter (OAuth PKCE) --------------------------------------------


@dataclass(frozen=True)
class PKCE:
    verifier: str = field(repr=False)
    challenge: str
    state: str = field(repr=False)


def new_pkce() -> PKCE:
    verifier = base64.urlsafe_b64encode(secrets.token_bytes(48)).rstrip(b"=").decode()
    challenge = (
        base64.urlsafe_b64encode(hashlib.sha256(verifier.encode()).digest()).rstrip(b"=").decode()
    )
    return PKCE(verifier=verifier, challenge=challenge, state=secrets.token_urlsafe(24))


def openrouter_auth_url(callback_url: str, pkce: PKCE) -> str:
    """The page the browser opens. OpenRouter takes a localhost callback on any port and
    returns ``code`` (and our ``state``) to it (openrouter.ai/docs, OAuth PKCE)."""
    q = {
        "callback_url": callback_url,
        "code_challenge": pkce.challenge,
        "code_challenge_method": "S256",
        "state": pkce.state,
    }
    return f"{OPENROUTER_AUTH_URL}?{urlencode(q)}"


async def openrouter_exchange(
    code: str, pkce: PKCE, *, client: httpx.AsyncClient | None = None
) -> str:
    """Trade the code for a key. Raises ValueError with a reason that holds no secret."""
    own = client is None
    http = client or httpx.AsyncClient(timeout=15.0)
    try:
        r = await http.post(
            OPENROUTER_KEYS_URL,
            json={"code": code, "code_verifier": pkce.verifier, "code_challenge_method": "S256"},
        )
    except httpx.HTTPError as e:
        raise ValueError(f"couldn’t reach OpenRouter ({type(e).__name__})") from None
    finally:
        if own:
            await http.aclose()
    if r.status_code >= 400:
        raise ValueError(f"OpenRouter refused the sign-in (HTTP {r.status_code})")
    try:
        key = r.json().get("key")
    except ValueError:
        key = None
    if not isinstance(key, str) or not key:
        raise ValueError("OpenRouter's answer had no key in it")
    return key
