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
    assert "<!-- ghostd setup" in text  # where the first run's detection commands will go


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
    # The comment marks where commands will be added later; `command -v` names binaries.
    text = re.sub(r"<!--.*?-->", "", guide(), flags=re.S)
    text = "\n".join(line for line in text.splitlines() if "command -v" not in line)
    used = set(re.findall(r"\bghostd ([a-z][a-z-]*)", text))
    assert used, "the guide names no ghostd command"
    assert used <= known, used - known
