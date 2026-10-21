"""Toolkits (ADR 0023): permission bundles that only narrow the policy file."""

from __future__ import annotations

import pytest
from ghost_core import Autonomy, AutonomyPolicy, Store, WriteGate
from ghost_core.policy import PolicyError
from test_mcp_writes import POLICY, connected, spec

TOOLKITS = """
[toolkit.finance]
description = "The books."
skills = ["month-close"]

[toolkit.finance.servers.github]
reads = ["issue_read"]
writes = ["comment", "set_labels"]
items = ["acme/app"]
levels = { set_labels = "confirm", comment = "deny" }

[toolkit.wide]
[toolkit.wide.servers.github]
"""


def policy() -> AutonomyPolicy:
    return AutonomyPolicy.parse(POLICY + TOOLKITS)


def test_parse_render_round_trip_and_validation():
    p = policy()
    t = p.toolkits["finance"]
    assert t.skills == ("month-close",) and t.grant("github").items == ("acme/app",)
    g = t.grant("github")
    assert g.allows_read("issue_read") and not g.allows_read("list_issues")
    again = AutonomyPolicy.parse(p.to_toml())
    assert again.toolkits == p.toolkits and again.mcp == p.mcp
    assert p.check_toolkits(skills=["month-close"]) == []
    bad = AutonomyPolicy.parse(
        POLICY
        + '[toolkit.bad]\nskills = ["nope"]\n[toolkit.bad.servers.github]\nitems = ["evil/corp"]\n'
        'writes = ["ghost_op", "launch"]\nlevels = { close = "quiet" }\n'
        "[toolkit.bad.servers.mars]\n"
    )
    problems = bad.check_toolkits(skills=["month-close"])
    assert any("evil/corp is not in github's scope items" in m for m in problems)
    assert any("declares no write launch" in m for m in problems)
    assert any("close = quiet is looser" in m for m in problems)
    assert any("server mars is not declared" in m for m in problems)
    assert any("no skill named nope" in m for m in problems)
    with pytest.raises(ValueError, match="unknown key"):
        AutonomyPolicy.parse("[toolkit.x]\nbogus = 1\n")
    with pytest.raises(ValueError, match="deny, confirm"):
        AutonomyPolicy.parse('[toolkit.x]\n[toolkit.x.servers.s]\nlevels = { a = "loud" }\n')


def test_a_toolkit_tightens_and_never_loosens():
    p = policy()
    assert p.resolve("github", "set_labels", "acme/app") is Autonomy.ANNOUNCE
    assert p.resolve("github", "set_labels", "acme/app", toolkit="finance") is Autonomy.CONFIRM
    assert p.resolve("github", "comment", "acme/app", toolkit="finance") is Autonomy.DENY
    # the file's deny on acme/web stays deny whatever the toolkit says
    assert p.resolve("github", "set_labels", "acme/web", toolkit="wide") is Autonomy.DENY
    assert p.resolve("github", "close", "acme/app", toolkit="unknown") is Autonomy.CONFIRM


async def test_a_grant_narrows_what_a_server_offers():
    from ghost_core.mcp import Servers, source_of

    p = policy()
    async with connected(spec()) as (server, world):
        full = source_of(server)
        view = source_of(server, p.toolkits["finance"].grant("github"), "finance")
        assert {t.name for t in full.read_tools} == {"github_issue_read", "github_list_issues"}
        assert [t.name for t in view.read_tools] == ["github_issue_read"]
        assert {o.name for o in view.operations} == {"comment", "set_labels"}
        assert view.toolkit == "finance" and view.scope_items() == ["acme/app"]
        assert "one repository, acme/app" in view.prompt and "github_list_issues" not in view.prompt
        # outside the grant's items: refused before the policy, on reads and writes
        read = view.read_tools[0]
        with pytest.raises(
            ValueError, match="not an allowed repository"
        ):  # the pipeline answers it
            await read.handler({"method": "get", "owner": "acme", "repo": "web", "issue_number": 7})
        labels = next(o for o in view.operations if o.name == "set_labels")
        with pytest.raises(ValueError, match="not an allowed repository"):
            await labels.prepare(
                {"owner": "acme", "repo": "web", "issue_number": 7, "labels": ["x"]}
            )
        # inside: the one item fills itself in, and the gate applies the toolkit's level
        prepared = await labels.prepare({"issue_number": 42, "labels": ["bug"]})
        assert prepared.target.scope() == "acme/app"
        gate = WriteGate(Store(), autonomy=p)
        gate.register("github", view.operations)
        assert (
            gate.level("github", "set_labels", prepared.target, toolkit="finance")
            is Autonomy.CONFIRM
        )
        sub = await gate.submit(
            session_id="s",
            ability="github",
            operation="set_labels",
            target=prepared.target,
            changes=prepared.changes,
            rationale="",
            toolkit="finance",
            department="accounting",
        )
        assert sub.autonomy is Autonomy.CONFIRM and sub.outcome is None
        assert sub.proposal.toolkit == "finance" and sub.proposal.department == "accounting"
        comment = next(o for o in view.operations if o.name == "comment")
        prepared = await comment.prepare({"issue_number": 42, "body": "hi"})
        with pytest.raises(PolicyError, match="not allowed"):
            await gate.submit(
                session_id="s",
                ability="github",
                operation="comment",
                target=prepared.target,
                changes=prepared.changes,
                rationale="",
                toolkit="finance",
            )
        # Servers.granted skips a server the toolkit names that is not connected
        servers = Servers({}, environ={})
        servers.servers["github"] = server
        servers.specs["github"] = server.spec
        assert [s.toolkit for s in servers.granted(p.toolkits["finance"])] == ["finance"]
        assert (
            servers.granted(
                AutonomyPolicy.parse("[toolkit.t]\n[toolkit.t.servers.mars]\n").toolkits["t"]
            )
            == []
        )
