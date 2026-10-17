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


def test_example_policy_parses():
    from pathlib import Path

    example = Path(__file__).parents[3] / "infra" / "policy.example.toml"
    p = AutonomyPolicy.parse(example.read_text())
    assert p.resolve("github_issues", "set_labels") is Autonomy.ANNOUNCE
    assert p.resolve("github_issues", "close") is Autonomy.CONFIRM


def test_abilities_section():
    p = AutonomyPolicy.parse('[abilities]\nenabled = ["notes"]\npassive = ["wake_onnx"]\n')
    assert p.enabled == ("notes",) and p.passive == ("wake_onnx",)
    assert AutonomyPolicy.parse("").enabled is None
    with pytest.raises(ValueError, match="abilities.enabled"):
        AutonomyPolicy.parse('[abilities]\nenabled = "notes"\n')
