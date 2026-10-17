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
