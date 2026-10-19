from ghost_core import ChangeSet, Store, WriteGate
from ghost_github_issues import AddComment, GitHubIssues, IssueTarget, RepoPolicy, SetLabels
from ghost_github_issues.mock import MOCK_REPOS, MockGitHubClient


def make():
    return GitHubIssues(MockGitHubClient(), RepoPolicy.from_csv(MOCK_REPOS))


async def test_mock_lists_and_reads():
    a = make()
    out = await a._list_awaiting_signoff({})
    assert out["count"] == 3
    issue = await a._get_issue({"repo": "ghost-mock/app", "number": 142})
    assert "Redis" in issue["title"]


async def test_mock_round_trip_through_gate_and_back():
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
    # taking it back is the opposite change, through the gate like any other
    back = await gate.submit(
        session_id="s",
        ability="github_issues",
        operation="set_labels",
        target=t,
        changes=ChangeSet(steps=(SetLabels(remove=("bug",)),)),
        rationale="operator asked to take it back",
    )
    assert back.executed and "bug" not in back.outcome.readback.labels


async def test_mock_state_survives_a_restart(tmp_path) -> None:
    from ghost_github_issues.mock import MockGitHubClient
    from ghost_github_issues.models import IssueTarget

    path = tmp_path / "mock-github.json"
    t = IssueTarget(owner="ghost-mock", repo="app", number=142)
    first = MockGitHubClient(path)
    await first.remove_label(t, "needs-signoff")
    await first.add_labels(t, ["approved"])
    await first.create_comment(t, "Approved.")

    again = MockGitHubClient(path)
    labels = {lb["name"] for lb in (await again.get_issue(t))["labels"]}
    assert "approved" in labels and "needs-signoff" not in labels
    assert (await again.list_comments(t))[-1]["body"] == "Approved."
    await again.create_comment(t, "Second.")
    ids = [c["id"] for c in await again.list_comments(t)]
    assert len(ids) == len(set(ids))
