"""Who publishes a catalog entry, whether it is the product maker's own, and where it runs
(#37). Deterministic: string rules over the registry's record, no model.

The registry checks one thing about every entry: its name's namespace.

- ``io.github.<owner>/…``: the publisher signed in to GitHub as ``<owner>``;
- ``com.example/…`` (reverse DNS): the publisher proved they own ``example.com``.

That says who published it, not that it is safe. Ghost builds on it:

- **The maker's own.** An entry is the product maker's own app when the publisher's name
  *is* the product's (``io.github.PostHog`` publishing "PostHog", ``com.notion`` publishing
  "Notion") and, for a GitHub account, something else agrees: it connects to a host of the
  same name (``mcp.posthog.com``), its repository is the publisher's, or its website is
  theirs. A domain is proof on its own. ``KNOWN_MAKERS`` is the short hand-kept list for
  makers whose names differ from their product's (``getsentry`` makes Sentry). The registry's
  ``_meta`` carries no maker flag (2026-10: only status and dates), so nothing is read from
  it here.
- **Where it runs.** A remote's host, and whether that host is the maker's, the
  publisher's, or someone else's; a package's registry and identifier.
- **The same product.** Entries whose titles normalize to the maker's own product name
  (``Posthog``, ``PostHog``) fold under the maker's entry in search results; nothing else
  does.

All of it is the registry's text shown to the operator. None of it reaches the model.
"""

from __future__ import annotations

import re
from dataclasses import dataclass
from functools import lru_cache
from typing import Literal

Verified = Literal["github", "domain"]
# What agrees that an entry is the maker's own: the publisher's name is the product's;
# Ghost's own list says so; it connects to the maker's host; its repository is theirs; its
# website is theirs.
Evidence = Literal["publisher", "listed", "remote", "repository", "website"]
HostOwner = Literal["maker", "publisher", "other"]

_GITHUB_NS = "io.github."


@dataclass(frozen=True)
class KnownMaker:
    product: str  # the product's name as its maker writes it
    brands: frozenset[str]  # the names on the maker's own hosts: "sentry" for mcp.sentry.dev


# Makers whose evidence is unusual: a GitHub account or a host not named like the product.
# Keyed by the registry namespace. Keep it short; every entry is a judgment someone checked.
KNOWN_MAKERS: dict[str, KnownMaker] = {
    "io.github.github": KnownMaker("GitHub", frozenset({"github", "githubcopilot"})),
    "io.github.getsentry": KnownMaker("Sentry", frozenset({"sentry", "getsentry"})),
    "io.github.makenotion": KnownMaker("Notion", frozenset({"notion", "makenotion"})),
}

# Hosts where anyone can run something under a name of their own: ``x.vercel.app`` is not
# Vercel's, so a host here has no brand and is never the maker's.
SHARED_HOSTS = (
    "vercel.app",
    "fly.dev",
    "workers.dev",
    "pages.dev",
    "herokuapp.com",
    "netlify.app",
    "onrender.com",
    "railway.app",
    "github.io",
    "githubusercontent.com",
    "run.app",
    "appspot.com",
    "web.app",
    "firebaseapp.com",
    "azurewebsites.net",
    "azurecontainerapps.io",
    "amazonaws.com",
    "cloudfront.net",
    "supabase.co",
    "deno.dev",
    "deno.net",
    "ngrok.io",
    "ngrok.app",
    "ngrok-free.app",
    "glitch.me",
    "replit.app",
    "repl.co",
    "modal.run",
    "hf.space",
    "trycloudflare.com",
)
# Second-level labels under a country code: ``foo.co.uk`` is foo's.
_SECOND_LEVEL = {"co", "com", "org", "net", "ac", "gov", "edu", "ne", "or", "go"}
# A GitHub account named for a company often carries one of these: ``pipeworx-io``,
# ``engageable-tech``. Used only to tell a publisher's own host, never for the maker's mark.
_ACCOUNT_SUFFIXES = {
    "io",
    "hq",
    "inc",
    "dev",
    "ai",
    "app",
    "labs",
    "official",
    "tech",
    "oss",
    "org",
    "com",
    "co",
    "team",
}


_SHARED = frozenset(SHARED_HOSTS)
_NOT_KEY = re.compile(r"[^a-z0-9]")
_ADDRESS = re.compile(r"[\d.]+|\[.*\]")


def key(text: str) -> str:
    """A name for comparing: lower case, letters and digits only. "PostHog" and "Posthog"
    are both ``posthog``."""
    return _NOT_KEY.sub("", text.lower())


@lru_cache(maxsize=8192)  # the index asks about ~50,000 hosts, many of them the same
def host_brand(host: str) -> str | None:
    """The name a host is registered under, as a key: ``mcp.posthog.com`` is ``posthog``,
    ``foo.co.uk`` is ``foo``. None for an address, a bare name, or a shared host
    (``x.vercel.app``), where the name isn't the owner's."""
    host = host.lower().strip(".").split(":")[0]
    if not host or _ADDRESS.fullmatch(host) or "." not in host:
        return None
    labels = host.split(".")
    if any(".".join(labels[i:]) in _SHARED for i in range(len(labels) - 1)):
        return None
    labels.pop()  # the top-level domain
    if len(labels) >= 2 and labels[-1] in _SECOND_LEVEL and len(host.rsplit(".", 1)[-1]) == 2:
        labels.pop()
    return key(labels[-1]) or None


# scheme://[user@]host[:port][/path]: the host, and the path's first part. A regex, not
# urlparse: the index reads ~50,000 addresses, and this is a third of the time.
_URL = re.compile(r"[a-zA-Z][a-zA-Z0-9+.-]*://(?:[^/?#@]*@)?([^/?#:]+)(?::\d*)?(?:/([^/?#]*))?")
_FORGES = ("github.com", "gitlab.com", "codeberg.org")


def url_host(url: str | None) -> str | None:
    """``https://mcp.posthog.com/mcp`` is ``mcp.posthog.com``; ``www.`` is dropped."""
    m = _URL.match(url or "")
    return m[1].lower().removeprefix("www.") if m else None


def repository_owner(url: str | None) -> str | None:
    """``https://github.com/PostHog/posthog`` is ``PostHog``; only GitHub, GitLab and
    Codeberg addresses, where the first part of the path is the owner."""
    m = _URL.match(url or "")
    if not m or m[1].lower().removeprefix("www.") not in _FORGES:
        return None
    return m[2] or None


@dataclass(frozen=True)
class Publisher:
    """Who published an entry, as the registry checked it."""

    namespace: str  # "io.github.PostHog", "com.mcparmory"
    name: str  # as shown: the GitHub account ("PostHog") or the domain ("mcparmory.com")
    verified: Verified
    brands: frozenset[str]  # names a host of theirs may carry: {"pipeworxio", "pipeworx"}


def publisher_of(name: str) -> Publisher | None:
    """From the registry name: ``io.github.PostHog/mcp`` was published by the GitHub
    account PostHog; ``com.mcparmory/posthog`` by whoever owns mcparmory.com."""
    ns, slash, _ = name.partition("/")
    if not slash or not ns:
        return None
    return _publisher(ns)


@lru_cache(maxsize=16384)  # one publisher often has many entries
def _publisher(ns: str) -> Publisher | None:
    if ns.startswith(_GITHUB_NS):
        owner = ns.removeprefix(_GITHUB_NS)
        if not owner or "." in owner:
            return None
        brands = {key(owner)}
        parts = owner.lower().split("-")
        if len(parts) > 1 and parts[-1] in _ACCOUNT_SUFFIXES:
            brands.add(key("-".join(parts[:-1])))
        return Publisher(ns, owner, "github", frozenset(b for b in brands if b))
    labels = ns.split(".")
    if len(labels) < 2 or not all(labels):
        return None
    domain = ".".join(reversed(labels)).lower()
    brand = host_brand(domain)
    return Publisher(ns, domain, "domain", frozenset({brand} if brand else ()))


def _title_of(name: str) -> str:
    """The product a registry name is about, from the part after the namespace:
    ``posthog-mcp-server`` is "posthog". Empty when that part says nothing
    (``io.github.PostHog/mcp``)."""
    tail = name.partition("/")[2].lower()
    for affix in ("-mcp-server", "-mcp", "_mcp", "-server"):
        tail = tail.removesuffix(affix)
    for affix in ("mcp-server-", "mcp-", "mcp_"):
        tail = tail.removeprefix(affix)
    return "" if tail in ("", "mcp", "server", "mcp-server") else tail


def product_keys(name: str, title: str) -> frozenset[str]:
    """The keys an entry's product goes by: its title, and the product its registry name
    names. "Atlassian Rovo" from ``com.atlassian/atlassian-mcp-server`` is
    ``atlassianrovo`` and ``atlassian``."""
    return frozenset(k for k in (key(title), key(_title_of(name))) if k)


@dataclass(frozen=True)
class Maker:
    """The entry is the product maker's own, and why Ghost thinks so."""

    product: str  # "PostHog"
    evidence: tuple[Evidence, ...]
    brands: frozenset[str]  # the names on the maker's own hosts
    keys: frozenset[str]  # what other entries for the same product are titled, as keys


def maker_of(
    name: str,
    title: str,
    *,
    hosts: tuple[str, ...] = (),
    repository: str | None = None,
    website: str | None = None,
    publisher: Publisher | None = None,
) -> Maker | None:
    """Whether an entry is its product maker's own. The publisher's name must be the
    product's (or the entry's namespace is in ``KNOWN_MAKERS``); a GitHub account then
    needs one more thing to agree: a host, the repository, or the website."""
    pub = publisher or publisher_of(name)
    if pub is None:
        return None
    keys = product_keys(name, title)
    listed = KNOWN_MAKERS.get(pub.namespace)
    if listed is not None:
        brands = listed.brands
        if not keys & (brands | {key(listed.product)}):
            return None  # the maker's, but about something else: no mark
        product = listed.product
    else:
        # Only the exact name: ``posthog-hq`` is not PostHog.
        brands = frozenset({key(pub.name)} if pub.verified == "github" else pub.brands)
        if not keys & brands:
            return None
        product = title
    evidence: list[Evidence] = ["listed" if listed else "publisher"]
    if any(host_brand(h) in brands for h in hosts):
        evidence.append("remote")
    owner = repository_owner(repository)
    if owner and (
        key(owner) in brands or (pub.verified == "github" and owner.lower() == pub.name.lower())
    ):
        evidence.append("repository")
    site = url_host(website)
    if site and host_brand(site) in brands:
        evidence.append("website")
    if pub.verified == "github" and not listed and len(evidence) == 1:
        return None  # a GitHub name alone is not enough
    # Others fold under it by its title, its product's name, or the maker's own name in the
    # registry name ("atlassian" for "Atlassian Rovo"); never by another product the
    # registry name happens to mention (``com.foo/posthog`` titled "Foo" is not PostHog's).
    group = {key(title), key(product)} | (keys & brands)
    return Maker(product, tuple(evidence), brands, frozenset(k for k in group if k))


def host_owner(host: str, *, maker: Maker | None, publisher: Publisher | None) -> HostOwner:
    """Whose host an entry connects to: the product maker's, the publisher's, or someone
    else's (a gateway, a shared host)."""
    brand = host_brand(host)
    if brand is None:
        return "other"
    if maker is not None and brand in maker.brands:
        return "maker"
    if publisher is not None and brand in publisher.brands:
        return "publisher"
    return "other"
