from ghost_domain import Store, WriteGate
from ghost_skills import load_skills
from ghost_skills.github_issues.mock import MockGitHubClient


async def test_mock_mode_needs_no_token_and_defaults_allowlist():
    (s,) = load_skills(["github_issues"], {"github_issues": {"mock": True}})
    assert isinstance(s._client, MockGitHubClient)
    assert "ghost-mock/app" in s.policy.allowed


async def test_mock_round_trip_through_gate():
    (s,) = load_skills(["github_issues"], {"github_issues": {"mock": True}})
    before = await s._list_awaiting_signoff({})
    assert before["count"] == 3
    req = await s.prepare("approve", {"repo": "ghost-mock/app", "number": 142, "comment": "Redis."})
    gate = WriteGate(Store(), s.writer, s.policy)
    await gate.prepare(
        session_id="s",
        skill=s.name,
        operation=req.operation,
        target=req.target,
        changes=req.changes,
        rationale="",
    )
    _, outcome = await gate.on_user_turn("s", "confirm")
    assert outcome.status == "success" and "approved" in outcome.readback.labels
    after = await s._list_awaiting_signoff({})
    assert after["count"] == 2
    comments = await s._list_comments({"repo": "ghost-mock/app", "number": 142})
    assert comments["comments"][-1]["by_ghost"] and "Redis." in comments["comments"][-1]["body"]


async def test_mock_delete_comment_by_id():
    from ghost_domain import Target
    from ghost_skills.github_issues.client import GitHubIssueWriter
    from ghost_skills.github_issues.mock import MockGitHubClient

    c = MockGitHubClient()
    w = GitHubIssueWriter(c)
    t = Target(owner="ghost-mock", repo="app", number=151)
    await w.add_comment(t, "hello", "ghost:abc")
    assert any("ghost:abc" in x["body"] for x in await c.list_comments(t))
    await w.delete_comment(t, "ghost:abc")
    assert not any("ghost:abc" in x["body"] for x in await c.list_comments(t))
    await w.delete_comment(t, "ghost:abc")  # idempotent


def test_ops_declare_risk():
    from ghost_domain import RepoPolicy
    from ghost_skills.github_issues.mock import MockGitHubClient
    from ghost_skills.github_issues.skill import GitHubIssuesSkill

    s = GitHubIssuesSkill(MockGitHubClient(), RepoPolicy.from_csv("ghost-mock/app"))
    risks = {op.name: op.risk for op in s.prepare_ops}
    assert risks["set_labels"] == "low" and risks["comment"] == "low"
    assert risks["close"] == "high" and risks["reopen"] == "medium"
    assert "change" in s.prompt_fragment and "word for word" not in s.prompt_fragment
