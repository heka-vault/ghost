"""The setup guide a person's coding agent follows (docs/agent-setup.md, heyghost.app/agents.md)
and the prompt that sends it there: one source each, the same words on the site and in the
README, and nothing in the guide that the daemon does not have. The site's own build checks
that /agents.md is the repository's file (site/scripts/verify-agents.mjs)."""

from __future__ import annotations

import re
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
GUIDE = ROOT / "docs/agent-setup.md"
PROMPT = ROOT / "site/src/agent-prompt.txt"
URL = "https://heyghost.app/agents.md"


def guide() -> str:
    return GUIDE.read_text()


def prompt() -> str:
    return PROMPT.read_text().strip()


def test_the_readme_carries_the_sites_prompt_word_for_word() -> None:
    readme = (ROOT / "README.md").read_text()
    assert f"```text\n{prompt()}\n```" in readme


def test_the_prompt_is_short_and_names_the_guide() -> None:
    p = prompt()
    assert "\n" not in p
    assert URL in p
    sentences = [s for s in re.split(r"(?<=[.!?])\s+", p) if s]
    assert len(sentences) <= 3, sentences


def test_the_site_serves_the_guide_from_the_one_file() -> None:
    endpoint = (ROOT / "site/src/pages/agents.md.ts").read_text()
    assert 'from "../../../docs/agent-setup.md?raw"' in endpoint
    assert URL in (ROOT / "site/src/pages/llms.txt.ts").read_text()


def test_the_guide_is_short_with_its_rules_at_both_ends() -> None:
    text = guide()
    assert len(text.splitlines()) <= 270
    assert text.count("## Never do these") == 2
    # ADR 0068: the AI tools already on this computer, by the commands `ghostd setup` has.
    assert "### Use what the person already has" in text
    for verb in ("detect", "import", "link", "unlink"):
        assert f"ghostd setup {verb}" in text, verb


def test_the_guide_never_pipes_a_download_into_a_shell_or_adds_a_repository() -> None:
    text = guide()
    for bad in (r"\|\s*(sudo\s+)?(ba)?sh\b", r"add-apt-repository", r"sources\.list", r">\s*/etc/"):
        for block in re.findall(r"```bash\n(.*?)```", text, re.S):
            assert not re.search(bad, block), (bad, block)


def test_every_ghostd_command_in_the_guide_is_one_the_daemon_has() -> None:
    main = (ROOT / "crates/ghostd/src/main.rs").read_text()
    listed = re.search(r"unknown command \{other:\?\}; commands: ([a-z, -]+)", main)
    assert listed, "main.rs no longer lists its commands"
    known = set(listed[1].split(", "))
    # An HTML comment is not an instruction; `command -v` names binaries.
    text = re.sub(r"<!--.*?-->", "", guide(), flags=re.S)
    text = "\n".join(line for line in text.splitlines() if "command -v" not in line)
    used = set(re.findall(r"\bghostd ([a-z][a-z-]*)", text))
    assert used, "the guide names no ghostd command"
    assert used <= known, used - known


def test_every_ghostd_setup_verb_in_the_guide_is_one_the_command_has() -> None:
    usage = (ROOT / "crates/ghostd/src/setup_cli.rs").read_text()
    line = re.search(r'pub const USAGE: &str = "([^"]+)"', usage)
    assert line, "setup_cli.rs no longer has its usage line"
    known = set(re.findall(r"\b(detect|import|link|unlink)\b", line[1]))
    used = set(re.findall(r"\bghostd setup ([a-z]+)", guide()))
    assert used <= known, used - known
    assert ("--yes" in line[1]) and ("--all" in line[1])


def test_the_guide_does_not_ask_the_person_to_wait_before_opening_ghost() -> None:
    """#135: the app follows a download the terminal started; only 2026.11.0 needs the wait."""
    text = guide()
    flat = " ".join(text.split())
    assert "Don't open Ghost" not in flat
    assert "open Ghost whenever they like" in flat
    waits = [p for p in text.split("\n\n") if "not to open Ghost" in " ".join(p.split())]
    assert len(waits) == 1 and "2026.11.0" in waits[0], waits
