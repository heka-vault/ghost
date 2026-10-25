import pytest
from ghost_core import Autonomy, AutonomyPolicy

FILE = """
[defaults]
autonomy = "confirm"

[github_issues]
set_labels = "announce"
comment = "quiet"

[github_issues.targets."Acme/Public"]
comment = "confirm"
close = "deny"

[notes]
"*" = "announce"
"""


def test_resolution_order():
    p = AutonomyPolicy.parse(FILE)
    # target beats skill
    assert p.resolve("github_issues", "comment", "acme/public") is Autonomy.CONFIRM
    assert p.resolve("github_issues", "comment", "acme/other") is Autonomy.QUIET
    # target rule for an op the skill section doesn't mention
    assert p.resolve("github_issues", "close", "acme/public") is Autonomy.DENY
    # skill rule
    assert p.resolve("github_issues", "set_labels", "acme/other") is Autonomy.ANNOUNCE
    # [defaults] beats risk class
    assert p.resolve("github_issues", "close", "acme/other", risk="low") is Autonomy.CONFIRM
    # wildcard
    assert p.resolve("notes", "append", "", risk="high") is Autonomy.ANNOUNCE


def test_missing_file_falls_back_to_risk(tmp_path):
    p = AutonomyPolicy.load(tmp_path / "nope.toml")
    assert "missing" in p.source
    assert p.resolve("x", "y", risk="low") is Autonomy.ANNOUNCE
    assert p.resolve("x", "y", risk="medium") is Autonomy.CONFIRM
    assert p.resolve("x", "y", risk="high") is Autonomy.CONFIRM


def test_load_reads_file(tmp_path):
    f = tmp_path / "policy.toml"
    f.write_text('[github_issues]\nclose = "announce"\n')
    assert AutonomyPolicy.load(f).resolve("github_issues", "close") is Autonomy.ANNOUNCE


@pytest.mark.parametrize(
    "text,needle",
    [
        ('[github_issues]\nclose = "yolo"\n', "yolo"),
        ("[defaults]\nautonomy = 3\n", "defaults.autonomy"),
        ("github_issues = 1\n", "must be a table"),
        ("[a\n", "<string>"),
    ],
)
def test_bad_files_name_the_problem(text, needle):
    with pytest.raises(ValueError, match=needle):
        AutonomyPolicy.parse(text)


def test_describe_lists_every_op():
    p = AutonomyPolicy.parse('[github_issues]\nclose = "deny"\n')
    out = p.describe("github_issues", {"close": "high", "comment": "low"})
    assert out == {"close": Autonomy.DENY, "comment": Autonomy.ANNOUNCE}


def test_example_layout_parses():
    """infra/example is what `make env` seeds: the policy file and Ghost's folder."""
    from pathlib import Path

    from ghost_core.departments import load_department

    infra = Path(__file__).parents[3] / "infra"
    p = AutonomyPolicy.parse((infra / "policy.example.toml").read_text())
    assert all(not ops for ops in p.skills.values()) and p.default is None
    ghost = load_department("ghost", infra / "example", is_ghost=True)
    assert ghost.problems == [] and set(ghost.servers) == {"github"}
    assert set(ghost.servers["github"].writes) == {"comment", "set_labels", "close", "reopen"}
    merged = ghost.policy(p)
    assert merged.resolve("github", "set_labels") is Autonomy.ANNOUNCE
    assert merged.resolve("github", "close") is Autonomy.CONFIRM
    assert ghost.rules_lead() == "Ghost"


def test_abilities_section_is_ignored():
    """The retired ability packages' section (ADR 0022) parses to nothing."""
    p = AutonomyPolicy.parse('[abilities]\nenabled = ["notes"]\n[notes]\nadd = "quiet"\n')
    assert p.resolve("notes", "add") is Autonomy.QUIET
    assert "[abilities]" not in p.to_toml()
