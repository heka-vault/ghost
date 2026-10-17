from ghost_core import ChangeSet, Store, WriteGate
from ghost_github_issues import AddComment, GitHubIssues, IssueTarget, RepoPolicy, SetLabels
from ghost_github_issues.client import GitHubIssueWriter
from ghost_github_issues.mock import MOCK_REPOS, MockGitHubClient


def make():
    return GitHubIssues(MockGitHubClient(), RepoPolicy.from_csv(MOCK_REPOS))


async def test_mock_lists_and_reads():
    a = make()
    out = await a._list_awaiting_signoff({})
    assert out["count"] == 3
    issue = await a._get_issue({"repo": "ghost-mock/app", "number": 142})
    assert "Redis" in issue["title"]


async def test_mock_round_trip_through_gate_announces_and_undoes():
    a = make()
    gate = WriteGate(Store())
    gate.register("github_issues", a.contributions().operations)
    t = IssueTarget(owner="ghost-mock", repo="app", number=151)
    sub = await gate.submit(
        session_id="s",
        ability="github_issues",
        operation="set_labels",
        target=t,
        changes=ChangeSet(steps=(SetLabels(add=("bug",)), AddComment(body="hi"))),
        rationale="",
    )
    assert sub.executed and sub.outcome.status == "success"
    assert "bug" in sub.outcome.readback.labels
    out = await gate.undo("s")
    assert out.status == "success" and "bug" not in out.readback.labels
    assert not any("hi" in c["body"] for c in await a._client.list_comments(t))


async def test_mock_delete_comment_by_id():
    from ghost_github_issues.models import DeleteComment

    c = MockGitHubClient()
    w = GitHubIssueWriter(c)
    t = IssueTarget(owner="ghost-mock", repo="app", number=151)
    await w.apply(t, AddComment(body="hello"), "ghost:abc")
    assert any("ghost:abc" in x["body"] for x in await c.list_comments(t))
    await w.apply(t, DeleteComment(marker="ghost:abc"), "k")
    assert not any("ghost:abc" in x["body"] for x in await c.list_comments(t))
    await w.apply(t, DeleteComment(marker="ghost:abc"), "k")  # idempotent
