"""Who publishes a catalog entry, whether it is the product maker's own, and where it runs
(#37). Deterministic: string rules over the registry's record, no model.

The registry checks one thing about every entry: its name's namespace.

- ``io.github.<owner>/…``: the publisher signed in to GitHub as ``<owner>``;
- ``com.example/…`` (reverse DNS): the publisher proved they own ``example.com``.

That says who published it, not that it is safe. Ghost builds on it:

- **The maker's own.** An entry is the product maker's own app when the publisher's name
  *is* the product's (``io.github.PostHog`` publishing "PostHog", ``com.notion`` publishing
  "Notion") and it connects to the maker's own host: for a domain, a host under that exact
  domain (``com.notion`` and ``mcp.notion.com``; ``ai.posthog`` is not ``posthog.com``); for
  a GitHub account, a host registered under the account's name (``mcp.posthog.com`` for
  ``PostHog``). The name alone is never enough, and neither is a repository or a website:
  every GitHub account owns repositories under its own name, and anyone can write any
  website. ``KNOWN_MAKERS`` is the short hand-kept list for makers whose names differ from
  their product's (``getsentry`` makes Sentry), or whose own app has no remote. The
  registry's ``_meta`` carries no maker flag (2026-10: only status and dates), so nothing is
  read from it here.
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
# Ghost's own list says so; it connects to the maker's host. Only "listed" or "remote" is
# proof; a domain publisher's repository under its name and a website on the maker's host
# are said beside it, never instead of it.
Evidence = Literal["publisher", "listed", "remote", "repository", "website"]
HostOwner = Literal["maker", "publisher", "other"]

_GITHUB_NS = "io.github."


@dataclass(frozen=True)
class KnownMaker:
    product: str  # the product's name as its maker writes it
    brands: frozenset[str]  # the names its entries go by: "sentry", "getsentry"
    domains: frozenset[str]  # the maker's own registered domains: "sentry.dev"


def _k(*brands: str) -> frozenset[str]:
    return frozenset(brands)


# Makers whose evidence is unusual: a GitHub account or a host not named like the product.
# Keyed by the registry namespace. Keep it short; every entry is a judgment someone checked.
KNOWN_MAKERS: dict[str, KnownMaker] = {
    # GitHub's own account; its app runs on api.githubcopilot.com.
    "io.github.github": KnownMaker(
        "GitHub",
        frozenset({"github", "githubcopilot"}),
        frozenset({"github.com", "githubcopilot.com"}),
    ),
    # Sentry's account is getsentry; its app runs on mcp.sentry.dev.
    "io.github.getsentry": KnownMaker(
        "Sentry", frozenset({"sentry", "getsentry"}), frozenset({"sentry.dev", "sentry.io"})
    ),
    # Notion's account is makenotion (``io.github.notion`` is not Notion).
    "io.github.makenotion": KnownMaker(
        "Notion", frozenset({"notion", "makenotion"}), frozenset({"notion.com", "notion.so"})
    ),
    # Neon proved neon.com; its app runs on mcp.neon.tech, Neon's other domain.
    "com.neon": KnownMaker("Neon", _k("neon"), frozenset({"neon.com", "neon.tech"})),
    # The makers below publish a package run on this computer, so no host of theirs can
    # agree. Each namespace was checked by hand on 2026-10-28 as the company's own account
    # or domain, and was among the real makers the stricter rule above unmarked.
    "com.auth0": KnownMaker("Auth0", _k("auth0"), frozenset({"auth0.com"})),
    "io.github.ChromeDevTools": KnownMaker(
        "Chrome DevTools", _k("chromedevtools"), frozenset({"chrome.com"})
    ),
    "io.github.ClickHouse": KnownMaker(
        "ClickHouse", _k("clickhouse"), frozenset({"clickhouse.com", "clickhouse.cloud"})
    ),
    "io.github.PagerDuty": KnownMaker("PagerDuty", _k("pagerduty"), frozenset({"pagerduty.com"})),
    "io.github.browserbase": KnownMaker(
        "Browserbase", _k("browserbase"), frozenset({"browserbase.com"})
    ),
    "io.github.buildkite": KnownMaker("Buildkite", _k("buildkite"), frozenset({"buildkite.com"})),
    "io.github.couchbase": KnownMaker("Couchbase", _k("couchbase"), frozenset({"couchbase.com"})),
    "io.github.firebase": KnownMaker("Firebase", _k("firebase"), frozenset({"firebase.com"})),
    "io.github.jfrog": KnownMaker("JFrog", _k("jfrog"), frozenset({"jfrog.com", "jfrog.io"})),
    "io.github.localstack": KnownMaker(
        "LocalStack", _k("localstack"), frozenset({"localstack.cloud"})
    ),
    "io.github.metabase": KnownMaker("Metabase", _k("metabase"), frozenset({"metabase.com"})),
    "io.github.paypal": KnownMaker("PayPal", _k("paypal"), frozenset({"paypal.com"})),
    "io.github.planetscale": KnownMaker(
        "PlanetScale", _k("planetscale"), frozenset({"planetscale.com", "pscale.dev"})
    ),
    "io.github.questdb": KnownMaker(
        "QuestDB", _k("questdb"), frozenset({"questdb.com", "questdb.io"})
    ),
    "io.github.upstash": KnownMaker(
        "Upstash", _k("upstash"), frozenset({"upstash.com", "upstash.io"})
    ),
    "io.github.zscaler": KnownMaker("Zscaler", _k("zscaler"), frozenset({"zscaler.com"})),
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
def registered_domain(host: str) -> str | None:
    """The domain a host is registered under: ``mcp.posthog.com`` is ``posthog.com``,
    ``api.foo.co.uk`` is ``foo.co.uk``. None for an address, a bare name, or a shared host
    (``x.vercel.app``), where the name isn't the owner's."""
    host = host.lower().strip(".").split(":")[0]
    if not host or _ADDRESS.fullmatch(host) or "." not in host:
        return None
    labels = host.split(".")
    if any(".".join(labels[i:]) in _SHARED for i in range(len(labels) - 1)):
        return None
    keep = 2
    if len(labels) >= 3 and labels[-2] in _SECOND_LEVEL and len(labels[-1]) == 2:
        keep = 3
    if not all(labels[-keep:]):
        return None
    return ".".join(labels[-keep:])


def host_brand(host: str) -> str | None:
    """The name a host is registered under, as a key: ``mcp.posthog.com`` is ``posthog``,
    ``foo.co.uk`` is ``foo``. None where ``registered_domain`` is."""
    domain = registered_domain(host)
    return (key(domain.split(".", 1)[0]) or None) if domain else None


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
    domains: frozenset[str]  # the maker's own registered domains: "posthog.com"
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
    product's, and the entry must connect to the maker's own host (a domain publisher's
    host under that exact domain; a GitHub account's host registered under its name), or
    its namespace is in ``KNOWN_MAKERS``. A repository or a website is said, never proof."""
    pub = publisher or publisher_of(name)
    if pub is None:
        return None
    keys = product_keys(name, title)
    listed = KNOWN_MAKERS.get(pub.namespace)
    domains: set[str] = set()
    if listed is not None:
        brands = listed.brands
        if not keys & (brands | {key(listed.product)}):
            return None  # the maker's, but about something else: no mark
        product = listed.product
        domains |= listed.domains

        def own(host: str) -> bool:
            return registered_domain(host) in listed.domains
    elif pub.verified == "github":
        # Only the exact name: ``posthog-hq`` is not PostHog.
        brands = frozenset({key(pub.name)})
        if not keys & brands:
            return None
        product = title

        def own(host: str) -> bool:
            return host_brand(host) in brands
    else:
        # A domain proves its owner, and only that domain: ``ai.posthog`` publishing
        # "PostHog" owns posthog.ai, which says nothing about posthog.com.
        brands = pub.brands
        if not keys & brands:
            return None
        product = title
        proven = registered_domain(pub.name)
        if proven is None:
            return None
        domains.add(proven)

        def own(host: str) -> bool:
            return registered_domain(host) == proven

    evidence: list[Evidence] = ["listed" if listed else "publisher"]
    mine = [d for h in hosts if own(h) and (d := registered_domain(h))]
    if mine:
        evidence.append("remote")
        domains.update(mine)
    elif listed is None:
        return None  # the publisher's name alone is not enough
    # Said beside the proof, never instead of it. A GitHub account's repository is always
    # its own, so for one it says nothing.
    owner = repository_owner(repository)
    if owner and pub.verified == "domain" and key(owner) in brands:
        evidence.append("repository")
    site = url_host(website)
    if site and own(site):
        evidence.append("website")
    # Others fold under it by its title, its product's name, or the maker's own name in the
    # registry name ("atlassian" for "Atlassian Rovo"); never by another product the
    # registry name happens to mention (``com.foo/posthog`` titled "Foo" is not PostHog's).
    group = {key(title), key(product)} | (keys & brands)
    return Maker(product, tuple(evidence), frozenset(domains), frozenset(k for k in group if k))


def host_owner(host: str, *, maker: Maker | None, publisher: Publisher | None) -> HostOwner:
    """Whose host an entry connects to: the product maker's (one of its own registered
    domains, exactly: ``posthog.ai`` is not PostHog's ``posthog.com``), the publisher's,
    or someone else's (a gateway, a shared host)."""
    domain = registered_domain(host)
    if domain is None:
        return "other"
    if maker is not None and domain in maker.domains:
        return "maker"
    if publisher is None:
        return "other"
    if publisher.verified == "domain":
        # The domain the registry checked, and nothing else under the same name.
        return "publisher" if domain == registered_domain(publisher.name) else "other"
    return "publisher" if host_brand(host) in publisher.brands else "other"
