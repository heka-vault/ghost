from fakes import AddComment, Issue, IssueSnap, SetLabels, changeset
from ghost_core import Proposal, Target
from ghost_core.codec import dump, dump_proposal, load, load_proposal


def test_round_trip_keeps_subclasses():
    t = Issue(owner="a", repo="b", number=1)
    assert load(dump(t), Target) == t
    p = Proposal(
        session_id="s",
        ability="x",
        operation="y",
        target=t,
        changes=changeset(SetLabels(add=("bug",)), AddComment(body="hi")),
        rationale="",
        snapshot=IssueSnap(version="v1", title="T", state="open", labels=()),
        preview="p",
    )
    back = load_proposal(dump_proposal(p))
    assert back == p
    assert type(back.changes.steps[1]) is AddComment


def test_load_refuses_wrong_base():
    import pytest

    d = dump(Issue(owner="a", repo="b", number=1))
    from ghost_core import Snapshot

    with pytest.raises(TypeError):
        load(d, Snapshot)


def test_retired_change_class_still_loads_for_the_audit():
    """Rows written by the removed undo feature carry step classes that no longer exist."""
    import json

    t = Issue(owner="a", repo="b", number=1)
    p = Proposal(
        session_id="s",
        ability="x",
        operation="y",
        target=t,
        changes=changeset(SetLabels(add=("bug",))),
        rationale="",
        snapshot=IssueSnap(version="v1", title="T", state="open", labels=()),
        preview="p",
    )
    body = json.loads(dump_proposal(p))
    body["operation"] = "undo:y"
    body["undo_of"] = "abc"
    body["changes"] = [{"$type": "fakes:DeleteComment", "data": {"marker": "ghost:abc"}}]
    from ghost_core.codec import RetiredChange

    back = load_proposal(json.dumps(body))
    assert back.operation == "undo:y"
    assert isinstance(back.changes.steps[0], RetiredChange)
    assert "no longer supported" in back.changes.describe()


def test_rows_from_a_retired_ability_still_read():
    """The GitHub ability is gone (ADR 0022); its audit rows must still load."""
    import json

    from ghost_core.codec import RetiredSnapshot, RetiredTarget, load_proposal

    row = {
        "id": "abc",
        "session_id": "s",
        "ability": "github_issues",
        "operation": "close",
        "rationale": "",
        "preview": "On acme/app#7: close the issue.",
        "autonomy": "confirm",
        "created_at": "2026-10-19T10:00:00Z",
        "expires_at": "2026-10-19T10:01:00Z",
        "status": "executed",
        "target": {
            "$type": "ghost_retired_ability.models:IssueTarget",
            "data": {"owner": "acme", "repo": "app", "number": 7},
        },
        "snapshot": {
            "$type": "ghost_retired_ability.models:IssueSnapshot",
            "data": {"version": "2026-10-19T09:00:00Z", "state": "open", "labels": ["bug"]},
        },
        "changes": [
            {"$type": "ghost_retired_ability.models:SetState", "data": {"state": "closed"}}
        ],
    }
    p = load_proposal(json.dumps(row))
    assert isinstance(p.target, RetiredTarget)
    assert p.target.spoken() == "acme/app#7" and p.target.scope() == "acme/app"
    assert isinstance(p.snapshot, RetiredSnapshot)
    assert p.snapshot.version == "2026-10-19T09:00:00Z"
    assert p.snapshot.spoken() == "open, labels bug"
    assert p.changes.describe().endswith("(no longer supported)")
