"""Who publishes a catalog entry, whether it is the product maker's own, where it runs, and
the folding of one product's entries under the maker's (#37). The fixture is the six
records Ghost's copy of the registry gave for "posthog" on 2026-10-28."""

from __future__ import annotations

import json
from pathlib import Path
from typing import Any

import pytest
from ghost_core.catalog import Catalog, parse_entry
from ghost_core.provenance import (
    host_brand,
    host_owner,
    maker_of,
    publisher_of,
    repository_owner,
    url_host,
)

POSTHOG_SIX: list[dict[str, Any]] = json.loads(
    (Path(__file__).parent / "fixtures" / "catalog_posthog.json").read_text()
)


def record(server: dict[str, Any]) -> dict[str, Any]:
    server = {"version": "1.0.0", "description": "", **server}
    return {"server": server, "_meta": {"io.modelcontextprotocol.registry/official": {}}}


def remote(url: str) -> list[dict[str, str]]:
    return [{"type": "streamable-http", "url": url}]


def npm(identifier: str) -> list[dict[str, Any]]:
    return [{"registryType": "npm", "identifier": identifier, "transport": {"type": "stdio"}}]


def catalog(tmp_path: Path, records: list[dict[str, Any]]) -> Catalog:
    path = tmp_path / "catalog.json"
    path.write_text(json.dumps({"records": {r["server"]["name"]: r for r in records}}))
    return Catalog(path)


def entry(rec: dict[str, Any]):
    e = parse_entry(rec)
    assert e is not None
    return e


# --- the namespace --------------------------------------------------------------------


@pytest.mark.parametrize(
    ("name", "shown", "verified"),
    [
        ("io.github.PostHog/mcp", "PostHog", "github"),
        ("io.github.pipeworx-io/posthog", "pipeworx-io", "github"),
        ("com.mcparmory/posthog", "mcparmory.com", "domain"),
        ("tech.engageable/analytics", "engageable.tech", "domain"),
        ("com.figma.mcp/mcp", "mcp.figma.com", "domain"),
    ],
)
def test_the_namespace_says_who_published_it(name, shown, verified):
    p = publisher_of(name)
    assert p is not None and (p.name, p.verified) == (shown, verified)


@pytest.mark.parametrize("name", ["nonamespace", "/x", "io.github./x", "io.github.a.b/x", "com/x"])
def test_a_name_without_a_namespace_has_no_publisher(name):
    assert publisher_of(name) is None


def test_a_company_account_on_github_may_run_hosts_under_its_name():
    assert publisher_of("io.github.pipeworx-io/posthog").brands == {"pipeworxio", "pipeworx"}
    assert publisher_of("io.github.PostHog/mcp").brands == {"posthog"}


@pytest.mark.parametrize(
    ("host", "brand"),
    [
        ("mcp.posthog.com", "posthog"),
        ("gateway.pipeworx.io", "pipeworx"),
        ("mcp.linear.app", "linear"),
        ("api.example.co.uk", "example"),
        ("example.io", "example"),
        ("someone.vercel.app", None),  # shared: the name is someone's, not Vercel's
        ("x.y.workers.dev", None),
        ("127.0.0.1", None),
        ("localhost", None),
    ],
)
def test_a_hosts_brand(host, brand):
    assert host_brand(host) == brand


def test_addresses_and_repositories():
    assert url_host("https://www.PostHog.com:443/mcp?x=1") == "posthog.com"
    assert url_host("https://user@mcp.posthog.com/mcp") == "mcp.posthog.com"
    assert url_host("not a url") is None and url_host(None) is None
    assert repository_owner("https://github.com/PostHog/posthog") == "PostHog"
    assert repository_owner("https://gitlab.com/acme/tool.git") == "acme"
    assert repository_owner("https://example.com/PostHog/posthog") is None
    assert repository_owner("https://github.com/") is None


# --- the six PostHog results ----------------------------------------------------------


def test_of_the_six_posthog_results_only_posthogs_own_is_the_makers():
    entries = {e.name: e for e in map(entry, POSTHOG_SIX)}
    assert len(entries) == 6
    own = entries["io.github.PostHog/mcp"]
    assert own.maker is not None and own.maker.product == "PostHog"
    assert own.maker.evidence == ("publisher", "remote", "repository")
    for name, e in entries.items():
        if name != own.name:
            assert e.maker is None, name
    who = {n: (e.publisher.name, e.publisher.verified) for n, e in entries.items()}
    assert who == {
        "io.github.PostHog/mcp": ("PostHog", "github"),
        "com.mcparmory/posthog": ("mcparmory.com", "domain"),
        "io.github.friendlygeorge/posthog-mcp-server": ("friendlygeorge", "github"),
        "io.github.mrfentmen/posthog-mcp": ("mrfentmen", "github"),
        "io.github.pipeworx-io/posthog": ("pipeworx-io", "github"),
        "tech.engageable/analytics": ("engageable.tech", "domain"),
    }


def test_where_each_posthog_result_runs():
    entries = {e.name: e for e in map(entry, POSTHOG_SIX)}
    own = entries["io.github.PostHog/mcp"]
    assert [o.host for o in own.options] == ["mcp.posthog.com"]
    assert host_owner("mcp.posthog.com", maker=own.maker, publisher=own.publisher) == "maker"
    gateway = entries["io.github.pipeworx-io/posthog"]
    assert [o.host for o in gateway.options] == ["gateway.pipeworx.io"]
    # Judged against PostHog: the publisher's own gateway, not PostHog's servers.
    owner = host_owner("gateway.pipeworx.io", maker=own.maker, publisher=gateway.publisher)
    assert owner == "publisher"
    packages = {
        n: [(o.registry, o.identifier) for o in e.options]
        for n, e in entries.items()
        if all(o.kind == "local" for o in e.options)
    }
    assert packages == {
        "com.mcparmory/posthog": [
            ("pypi", "mcparmory-posthog"),
            ("oci", "ghcr.io/mcparmory/posthog:1.0.5"),
        ],
        "io.github.friendlygeorge/posthog-mcp-server": [("npm", "posthog-mcp-server")],
        "io.github.mrfentmen/posthog-mcp": [("npm", "posthog-mcp")],
        "tech.engageable/analytics": [("pypi", "engageable")],
    }
    assert own.updated == "2026-01-07T16:34:40.847754Z"


def test_the_six_fold_under_posthogs_own_and_it_comes_first(tmp_path):
    cat = catalog(tmp_path, POSTHOG_SIX)
    found, total = cat.search("posthog")
    assert total == 2
    lead, other = found
    assert lead.entry.name == "io.github.PostHog/mcp"
    assert sorted(o.name for o in lead.others) == [
        "com.mcparmory/posthog",
        "io.github.friendlygeorge/posthog-mcp-server",
        "io.github.mrfentmen/posthog-mcp",
        "io.github.pipeworx-io/posthog",
    ]
    # "Engageable Analytics" mentions PostHog; it is another product, so it stands alone.
    assert other.entry.name == "tech.engageable/analytics" and other.others == ()
    pipeworx = cat.get("io.github.pipeworx-io/posthog")
    assert cat.product_maker(pipeworx) is lead.entry
    assert cat.product_maker(lead.entry) is lead.entry
    assert cat.product_maker(cat.get("tech.engageable/analytics")) is None


def test_without_the_makers_own_nothing_folds(tmp_path):
    cat = catalog(
        tmp_path, [r for r in POSTHOG_SIX if r["server"]["name"] != "io.github.PostHog/mcp"]
    )
    found, total = cat.search("posthog")
    assert total == 5 and all(f.others == () for f in found)


# --- the maker's own: edge cases ------------------------------------------------------


def test_a_github_name_alone_is_not_enough():
    """``io.github.PostHog`` publishing a package with no repository, host or website:
    nothing agrees with the name, so no mark."""
    assert maker_of("io.github.PostHog/tool", "PostHog") is None
    assert maker_of("io.github.PostHog/tool", "PostHog", repository="https://github.com/PostHog/x")


def test_a_proven_domain_is_enough():
    m = maker_of("com.notion/mcp", "Notion")
    assert m is not None and m.evidence == ("publisher",)
    m = maker_of("com.notion/mcp", "Notion", hosts=("mcp.notion.com",))
    assert m is not None and m.evidence == ("publisher", "remote")


def test_the_publishers_name_must_be_the_products():
    assert maker_of("com.mcparmory/posthog", "Posthog", hosts=("mcparmory.com",)) is None
    # A look-alike account is not the maker, whatever its repository says.
    look_alike = maker_of(
        "io.github.posthog-hq/posthog",
        "PostHog",
        repository="https://github.com/posthog-hq/posthog",
        hosts=("mcp.posthog-hq.dev",),
    )
    assert look_alike is None
    # The title or the product the name names: "Atlassian Rovo" by atlassian.com.
    m = maker_of("com.atlassian/atlassian-mcp-server", "Atlassian Rovo")
    assert m is not None and m.product == "Atlassian Rovo" and "atlassian" in m.keys
    # The maker's own app named for another product claims nothing of that product.
    foo = maker_of("com.foo/posthog", "Foo", hosts=("mcp.foo.com",))
    assert foo is not None and foo.keys == {"foo"}


def test_the_makers_own_on_someone_elses_host():
    """PostHog's own entry served from a gateway: still PostHog's (its repository agrees),
    but the host is not PostHog's."""
    m = maker_of(
        "io.github.PostHog/mcp",
        "PostHog",
        hosts=("gateway.pipeworx.io",),
        repository="https://github.com/PostHog/posthog",
    )
    assert m is not None and m.evidence == ("publisher", "repository")
    pub = publisher_of("io.github.PostHog/mcp")
    assert host_owner("gateway.pipeworx.io", maker=m, publisher=pub) == "other"


def test_a_shared_host_is_nobodys_brand():
    m = maker_of("io.github.vercel-fan/vercel", "Vercel", hosts=("vercel-fan.vercel.app",))
    assert m is None
    pub = publisher_of("io.github.vercel-fan/vercel")
    assert host_owner("vercel-fan.vercel.app", maker=None, publisher=pub) == "other"


def test_ghosts_own_list_names_makers_with_unusual_names():
    sentry = maker_of(
        "io.github.getsentry/sentry-mcp",
        "Sentry",
        hosts=("mcp.sentry.dev",),
        repository="https://github.com/getsentry/sentry-mcp",
    )
    assert sentry is not None and sentry.product == "Sentry"
    assert sentry.evidence == ("listed", "remote", "repository")
    github = maker_of(
        "io.github.github/github-mcp-server", "GitHub", hosts=("api.githubcopilot.com",)
    )
    assert github is not None and github.evidence == ("listed", "remote")
    # The maker's, but about another product: no mark.
    assert maker_of("io.github.github/spaces", "Copilot Spaces") is None


# --- grouping -------------------------------------------------------------------------


def test_folding_is_by_exact_title_only(tmp_path):
    linear = record(
        {
            "name": "app.linear/linear",
            "title": "Linear",
            "remotes": remote("https://mcp.linear.app/mcp"),
        }
    )
    wrapper = record({"name": "io.github.someone/linear-mcp", "packages": npm("linear-mcp")})
    by_them = record(
        {
            "name": "io.usefulapi/linear",
            "title": "Linear by usefulapi",
            "remotes": remote("https://linear.usefulapi.io/mcp"),
        }
    )
    project = record(
        {
            "name": "org.openlyuseful/linear-project",
            "title": "Linear Project",
            "packages": npm("lp"),
        }
    )
    weather_a = record({"name": "io.github.a/weather", "packages": npm("weather-a")})
    weather_b = record({"name": "io.github.b/weather", "packages": npm("weather-b")})
    cat = catalog(tmp_path, [linear, wrapper, by_them, project, weather_a, weather_b])
    found, total = cat.search("linear")
    assert [f.entry.name for f in found][0] == "app.linear/linear"
    assert [o.name for o in found[0].others] == ["io.github.someone/linear-mcp"]
    assert total == 3  # Linear with its wrapper, "Linear by usefulapi", "Linear Project"
    # Two entries titled alike with no maker's own among them stay apart.
    found, total = cat.search("weather")
    assert total == 2 and all(f.others == () for f in found)
    # A gateway for Linear is judged against Linear's own.
    usefulapi = cat.get("io.usefulapi/linear")
    assert cat.product_maker(usefulapi) is None  # its title isn't exactly Linear's


def test_searching_github_does_not_find_every_github_account(tmp_path):
    notes = record({"name": "io.github.someone/notes", "title": "Notes", "packages": npm("notes")})
    gh = record(
        {
            "name": "io.github.github/github-mcp-server",
            "title": "GitHub",
            "remotes": remote("https://api.githubcopilot.com/mcp/"),
        }
    )
    cat = catalog(tmp_path, [notes, gh])
    found, total = cat.search("github")
    assert total == 1 and found[0].entry.name == "io.github.github/github-mcp-server"
    assert cat.search("someone")[1] == 1  # the account still counts
