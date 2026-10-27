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


def test_the_older_layout_reads_as_rules_and_the_most_careful_wins():
    """ADR 0034: a per-source table becomes rules; ``[defaults] autonomy`` the mode."""
    p = AutonomyPolicy.parse(FILE)
    public, other = {"target": "acme/public"}, {"target": "acme/other"}
    # an ask for one target beats a quiet for every target
    assert p.resolve("github_issues", "comment", public) is Autonomy.CONFIRM
    assert p.resolve("github_issues", "comment", other) is Autonomy.QUIET
    assert p.resolve("github_issues", "close", public) is Autonomy.DENY
    assert p.resolve("github_issues", "set_labels", other) is Autonomy.ANNOUNCE
    # what no rule names takes the mode, here "ask" from defaults.autonomy
    assert p.resolve("github_issues", "close", other) is Autonomy.CONFIRM
    assert p.resolve("notes", "append") is Autonomy.ANNOUNCE  # the wildcard


def test_a_missing_file_runs_everything(tmp_path):
    p = AutonomyPolicy.load(tmp_path / "nope.toml")
    assert "missing" in p.source
    assert p.resolve("x", "y") is Autonomy.ANNOUNCE


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


def test_example_layout_parses():
    """infra/example is what `make env` seeds: the policy file and Ghost's folder."""
    from pathlib import Path

    from ghost_core.apps import APPS_FILE, load_apps
    from ghost_core.folder import GhostFolder
    from ghost_core.permissions import Mode

    infra = Path(__file__).parents[3] / "infra"
    p = AutonomyPolicy.parse((infra / "policy.example.toml").read_text())
    assert p.permissions.mode is Mode.OPEN and not any(p.permissions.rules().values())
    assert p.resolve("github", "issue_write") is Autonomy.ANNOUNCE
    ghost = GhostFolder(infra / "example")
    assert ghost.problems == []  # nothing an older layout left, nothing broken
    assert ghost.rules().startswith("# Ghost")
    apps = load_apps(infra / "example" / APPS_FILE)
    assert apps.problems == () and set(apps.apps) == {"github"}
    assert not (infra / "templates").exists()  # ADR 0035: no department folders to copy


def test_abilities_section_is_ignored():
    """The retired ability packages' section (ADR 0022) parses to nothing."""
    p = AutonomyPolicy.parse('[abilities]\nenabled = ["notes"]\n[notes]\nadd = "quiet"\n')
    assert p.resolve("notes", "add") is Autonomy.QUIET
    assert "[abilities]" not in p.to_toml()
