"""``GET /mcp`` (ADR 0022): each server from the policy file, its state and its tools."""

from __future__ import annotations

import asyncio

from ghost_core.mcp import Server, Servers, ServerSpec, ToolInfo


def test_mcp_route_lists_servers_and_tools(world):
    client, *_, deps = world
    proxy = ServerSpec(
        name="proxy", url="http://127.0.0.1:9", headers={"Authorization": "Bearer $NOPE"}
    )
    servers = Servers({"proxy": proxy}, environ={})
    asyncio.run(servers.connect())  # fails: its secret is not set
    # A connected server, by hand: no network in this test.
    fake = ServerSpec(name="fake", url="memory://", reads=("lookup",))
    live = Server(fake, environ={})
    live.tools = [
        ToolInfo("lookup", "Find things.", {}, read_only=True),
        ToolInfo("create", "Make a thing.", {}, read_only=False),
    ]
    servers.specs["fake"] = fake
    servers.servers["fake"] = live
    deps.mcp = servers

    body = client.get("/mcp").json()
    by_name = {s["name"]: s for s in body}
    assert by_name["proxy"] == {
        "name": "proxy",
        "transport": "http",
        "connected": False,
        "error": "secret NOPE is not set",
        "tools": [],
        "writes": [],
        "scope": None,
    }
    assert by_name["fake"]["connected"] is True
    assert by_name["fake"]["error"] is None
    assert [(t["name"], t["offered"]) for t in by_name["fake"]["tools"]] == [
        ("lookup", True),
        ("create", False),
    ]


def test_mcp_route_without_servers(world):
    client, *_ = world
    assert client.get("/mcp").json() == []
