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
