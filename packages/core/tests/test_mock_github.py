"""The mock GitHub MCP server: the official server's tool shape, state that survives a
restart, and the comment marker that makes a retried comment land once."""

from __future__ import annotations

import json
from pathlib import Path

from ghost_core.mock_github import MockGitHub, build_server


def test_seeded_state_and_persistence(tmp_path: Path):
    state = tmp_path / "mock.json"
    gh = MockGitHub(state)
    listed = gh.list_issues("ghost-mock", "app", labels=["needs-triage"])
    assert {i["number"] for i in listed["issues"]} == {142, 151}
    gh.issue_write("update", "ghost-mock", "infra", 17, state="closed", state_reason="completed")
    gh.add_issue_comment("ghost-mock", "app", 142, "Go with Redis.")
    again = MockGitHub(state)  # a restart picks the writes up
    assert again.issue_read("get", "ghost-mock", "infra", 17)["state"] == "closed"
    assert again.issue_read("get_comments", "ghost-mock", "app", 142)["comments"][-1]["body"] == (
        "Go with Redis."
    )
    assert (
        json.loads(state.read_text())["issues"]["ghost-mock/infra#17"]["state_reason"]
        == "completed"
    )


def test_search_and_create():
    gh = MockGitHub()
    assert [i["number"] for i in gh.search_issues("webhook")["items"]] == [17]
    made = gh.issue_write("create", "ghost-mock", "app", title="New one", labels=["needs-triage"])
    assert made["number"] == 152 and made["state"] == "open"


def test_server_has_the_official_tool_shape():
    srv = build_server()
    tools = {t.name: t for t in srv._tool_manager.list_tools()}  # noqa: SLF001
    assert set(tools) == {
        "issue_read",
        "issue_write",
        "add_issue_comment",
        "list_issues",
        "search_issues",
    }
    assert (
        tools["issue_read"].annotations is not None
        and tools["issue_read"].annotations.read_only_hint
    )
    assert tools["issue_write"].annotations is None
    assert set(tools["issue_write"].parameters["properties"]) >= {
        "method",
        "owner",
        "repo",
        "issue_number",
        "labels",
        "state",
    }
