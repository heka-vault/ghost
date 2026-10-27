"""Permissions (ADR 0034): everything runs unless a rule blocks it or asks first."""

import pytest
from ghost_core.levels import Autonomy
from ghost_core.permissions import Mode, Permissions, Rule
from ghost_core.policy import AutonomyPolicy


def perms(**lists) -> Permissions:
    return Permissions.parse(lists, "test")


def test_an_empty_policy_runs_every_change_and_announces_it():
    p = Permissions()
    assert p.resolve("github", "issue_write") is Autonomy.ANNOUNCE
    assert p.offers_change("github", "delete_repository")
    assert p.offers_read("github", "list_issues")


def test_block_beats_ask_beats_quiet_beats_allow():
    p = perms(
        block=["github.delete_*"],
        ask=["github.*"],
        quiet=["github.add_*", "github.delete_*"],
        allow=["github.*"],
    )
    assert p.resolve("github", "delete_branch") is Autonomy.DENY
    assert p.resolve("github", "add_issue_comment") is Autonomy.CONFIRM
    assert (
        perms(quiet=["github.add_*"], allow=["github.*"]).resolve("github", "add_issue_comment")
        is Autonomy.QUIET
    )


def test_a_whole_app_is_one_word_and_names_are_case_insensitive():
    p = perms(block=["Gmail"])
    assert p.resolve("gmail", "send_email") is Autonomy.DENY
    assert not p.offers_read("gmail", "search")
    assert p.offers_read("github", "list_issues")


def test_argument_patterns_decide_on_the_call_and_never_take_the_tool_away():
    p = perms(ask=["github.issue_write(state=closed)"], block=["github.*(repo!=site)"])
    assert p.offers_change("github", "issue_write")
    assert p.resolve("github", "issue_write", {"repo": "site", "state": "closed"}) is (
        Autonomy.CONFIRM
    )
    assert p.resolve("github", "issue_write", {"repo": "site", "state": "open"}) is (
        Autonomy.ANNOUNCE
    )
    # != also stops a call that leaves the argument out
    assert p.resolve("github", "search_issues", {"query": "bug"}) is Autonomy.DENY
    assert p.blocks("github", "list_issues", {"repo": "other"})
    assert not p.blocks("github", "list_issues", {"repo": "site"})


def test_a_list_argument_matches_when_any_item_does():
    p = perms(ask=["github.issue_write(labels=wont*)"])
    assert p.resolve("github", "issue_write", {"labels": ["bug", "wontfix"]}) is (Autonomy.CONFIRM)
    assert p.resolve("github", "issue_write", {"labels": ["bug"]}) is Autonomy.ANNOUNCE


def test_lockdown_modes():
    ask = perms(mode="ask", allow=["github.add_issue_comment"])
    assert ask.resolve("github", "issue_write") is Autonomy.CONFIRM
    assert ask.resolve("github", "add_issue_comment") is Autonomy.ANNOUNCE
    ro = perms(mode="read-only", allow=["github.issue_write(repo=site)"])
    assert ro.resolve("github", "add_issue_comment") is Autonomy.DENY
    assert not ro.offers_change("github", "add_issue_comment")
    # a conditional allow keeps the tool offered; the call decides
    assert ro.offers_change("github", "issue_write")
    assert ro.resolve("github", "issue_write", {"repo": "site"}) is Autonomy.ANNOUNCE
    assert ro.resolve("github", "issue_write", {"repo": "x"}) is Autonomy.DENY
    assert ro.offers_read("github", "list_issues")


@pytest.mark.parametrize("bad", ["github.(x)", "github.issue_write(state)", "", "a b"])
def test_a_rule_that_does_not_parse_is_an_error(bad):
    with pytest.raises(ValueError):
        Rule.parse(bad)


def test_rules_round_trip_through_their_text():
    for text in ["gmail", "github.delete_*", "github.issue_write(state=closed, repo!=site)"]:
        assert str(Rule.parse(text)) == text


def test_with_rule_moves_a_rule_between_lists():
    p = perms(ask=["github.issue_write"])
    moved = p.with_rule(Rule.parse("github.issue_write"), "block")
    assert moved.rules()["ask"] == [] and moved.rules()["block"] == ["github.issue_write"]
    assert moved.with_rule(Rule.parse("github.issue_write"), None).rules()["block"] == []


def test_the_policy_file_reads_permissions_and_round_trips():
    text = """
[permissions]
mode = "ask"
block = ["gmail.delete_*"]
allow = ["github.add_issue_comment"]

[defaults]
pending_ttl = 600
"""
    p = AutonomyPolicy.parse(text)
    assert p.permissions.mode is Mode.ASK
    assert p.resolve("gmail", "delete_draft") is Autonomy.DENY
    assert p.pending_ttl == 600
    again = AutonomyPolicy.parse(p.to_toml())
    assert again.permissions == p.permissions and again.pending_ttl == 600


def test_the_older_per_source_tables_still_read_as_rules():
    p = AutonomyPolicy.parse(
        """
[defaults]
autonomy = "confirm"

[conversations]
delete = "deny"
rename = "quiet"

[github.targets."acme/public"]
close = "deny"
"""
    )
    assert p.permissions.mode is Mode.ASK
    assert p.resolve("conversations", "delete") is Autonomy.DENY
    assert p.resolve("conversations", "rename") is Autonomy.QUIET
    assert p.resolve("conversations", "create") is Autonomy.CONFIRM
    assert p.resolve("github", "close", {"target": "acme/public"}) is Autonomy.DENY
    assert p.resolve("github", "close", {"target": "acme/app"}) is Autonomy.CONFIRM


def test_a_missing_file_is_open(tmp_path):
    p = AutonomyPolicy.load(tmp_path / "policy.toml")
    assert p.resolve("anything", "at_all") is Autonomy.ANNOUNCE


def test_writing_permissions_keeps_the_rest_of_the_file():
    from ghost_core.policy import write_permissions

    text = """# my notes
# keep these

[defaults]
autonomy = "confirm"
pending_ttl = 600

[conversations]
delete = "deny"

[routes]
enabled = ["digest", "phone"]  # mine
"""
    before = AutonomyPolicy.parse(text)
    perms_now = before.permissions.with_rule(Rule.parse("gmail"), "block")
    out = write_permissions(text, perms_now)
    assert out.startswith("# my notes\n# keep these\n")
    assert 'enabled = ["digest", "phone"]  # mine' in out
    assert "[conversations]" not in out and "autonomy" not in out
    after = AutonomyPolicy.parse(out)
    assert after.permissions == perms_now
    assert after.pending_ttl == 600
    assert after.resolve("conversations", "delete") is Autonomy.DENY
    # a second write replaces the table in place
    again = write_permissions(out, after.permissions.with_rule(Rule.parse("gmail"), None))
    assert again.count("[permissions]") == 1
    assert AutonomyPolicy.parse(again).resolve("gmail", "send") is Autonomy.CONFIRM
