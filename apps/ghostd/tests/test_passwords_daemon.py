"""The daemon's side of the vault (ADR 0036): opened at start with plaintext files still
read until the import, the waiting list's items, the log redacted, and
``ghostd passwords``. A fake keyring, temp folders and a dict for the environment: never
the operator's own."""

from __future__ import annotations

import io
import sys
from types import SimpleNamespace

import pytest
from ghost_core import passwords as pw
from ghost_core.passwords import MemoryKeyring, Passwords, child_env
from ghostd import passwords as gp
from loguru import logger

TOKEN = "sk-ant-api03-the-vaults-own-value"
OLD = "sk-ant-api03-the-plaintext-copy"


@pytest.fixture(autouse=True)
def _clean_redactor():
    yield
    pw._REDACTOR._sets.clear()
    pw._REDACTOR._pairs = ()


def cfg_for(tmp_path):
    config = tmp_path / "config"
    config.mkdir(exist_ok=True)
    return SimpleNamespace(
        data_dir=tmp_path / "data",
        config_dir=config,
        vault_env_path=tmp_path / "no-heka",
        api_socket=tmp_path / "no-daemon.sock",
    )


def test_open_reads_the_vault_over_plaintext_files_until_the_import(tmp_path):
    cfg = cfg_for(tmp_path)
    (cfg.config_dir / "env").write_text(
        f"ANTHROPIC_AUTH_TOKEN={OLD}\nANTHROPIC_BASE_URL=http://proxy\nMCP_PROXY_KEY=proxy-0123456789\n"
    )
    ring = MemoryKeyring()
    seeded = Passwords(cfg.data_dir / "passwords.vault", ring)
    assert seeded.open()
    seeded.set("ANTHROPIC_AUTH_TOKEN", TOKEN)

    env: dict[str, str] = {"PATH": "/bin"}
    vault = gp.open_passwords(cfg, keyring=ring, environ=env)
    assert vault.is_open
    assert env["ANTHROPIC_AUTH_TOKEN"] == TOKEN  # the vault's wins
    assert env["MCP_PROXY_KEY"] == "proxy-0123456789"  # not imported yet: still read
    assert env["ANTHROPIC_BASE_URL"] == "http://proxy"
    # Both are passwords to every check: redacted, and never handed to a child process.
    assert pw.redact_text(f"a {OLD} b proxy-0123456789") == (
        "a [password: ANTHROPIC_AUTH_TOKEN] b [password: MCP_PROXY_KEY]"
    )
    assert set(child_env(env)) == {"PATH", "ANTHROPIC_BASE_URL"}
    keys = [k for k, _ in gp.problems(vault, cfg)]
    assert keys == ["passwords:import"]


def test_a_locked_keyring_is_on_the_waiting_list(tmp_path):
    cfg = cfg_for(tmp_path)
    ring = MemoryKeyring()
    seeded = Passwords(cfg.data_dir / "passwords.vault", ring)
    assert seeded.open()
    seeded.set("HF_TOKEN", "hf_0123456789abcdef")
    ring.locked = True
    vault = gp.open_passwords(cfg, keyring=ring, environ={})
    assert vault.state == "locked"
    assert gp.problems(vault, cfg) == [
        ("passwords:locked", "Unlock your keyring so Ghost can read its passwords")
    ]
    from ghostd.main import ghost_problems

    apps = SimpleNamespace(specs={})
    out = ghost_problems(apps, SimpleNamespace(problems=[]), gp.problems(vault, cfg))  # type: ignore[arg-type]
    assert out[0][0] == "passwords:locked"


async def test_the_daemon_follows_a_vault_that_opens_late(tmp_path, monkeypatch):
    from ghost_voice import VoiceSettings
    from ghostd.config import GhostdSettings
    from ghostd.main import Daemon

    ring = MemoryKeyring()
    seeded = Passwords(tmp_path / "passwords.vault", ring)
    assert seeded.open()
    seeded.set("OPENROUTER_API_KEY", "sk-or-0123456789abcdef")
    ring.locked = True
    env: dict[str, str] = {}
    vault = Passwords(tmp_path / "passwords.vault", ring, environ=env)
    assert not vault.open()

    d = Daemon(GhostdSettings(data_dir=tmp_path), VoiceSettings.model_construct(), vault)
    revived: list[str] = []

    async def revive(name):
        revived.append(name)
        return True

    d.apps = SimpleNamespace(failed={"proxy": "setting PROXY_KEY is not set"}, revive=revive)  # type: ignore[assignment]
    swapped = []
    d.gate = SimpleNamespace(set_classifier=swapped.append)  # type: ignore[assignment]
    monkeypatch.setenv("OPENROUTER_API_KEY", "sk-or-0123456789abcdef")
    monkeypatch.setattr(gp, "VAULT_RETRY_SECS", 0.01)
    monkeypatch.setattr("ghostd.main.VAULT_RETRY_SECS", 0.01)
    ring.locked = False
    await d._wait_for_vault()
    assert vault.is_open and env["OPENROUTER_API_KEY"].startswith("sk-or-")
    assert revived == ["proxy"]
    assert swapped and type(swapped[0]).__name__ == "JevClassifier"
    for j in d._jevs:
        await j.aclose()


def test_no_log_line_carries_a_password(tmp_path):
    vault = Passwords(tmp_path / "passwords.vault", MemoryKeyring())
    assert vault.open()
    vault.set("ANTHROPIC_AUTH_TOKEN", TOKEN)
    out = io.StringIO()
    logger.configure(patcher=gp.redacting)  # type: ignore[arg-type]
    sink = logger.add(out, format="{message}")
    try:
        logger.warning(f"the endpoint said: bad token {TOKEN}")
    finally:
        logger.remove(sink)
        logger.configure(patcher=None)  # type: ignore[arg-type]
    assert TOKEN not in out.getvalue()
    assert "bad token [password: ANTHROPIC_AUTH_TOKEN]" in out.getvalue()


def test_in_use_follows_the_model_connection_and_the_voice():
    from ghost_core.connection import ModelConnection

    def voice(tts: str, **env: str) -> SimpleNamespace:
        conn = ModelConnection.from_env(env)
        return SimpleNamespace(llm_connection=lambda: conn, tts_engine=tts)

    proxy = voice("kokoro", ANTHROPIC_BASE_URL="http://x:8317", ANTHROPIC_AUTH_TOKEN="t")
    assert gp.in_use(proxy) == ["ANTHROPIC_AUTH_TOKEN"]
    assert gp.in_use(voice("elevenlabs", GHOST_LLM_ENDPOINT="openrouter")) == [
        "OPENROUTER_API_KEY",
        "ELEVENLABS_API_KEY",
    ]
    assert gp.in_use(voice("kokoro", GHOST_LLM_PROVIDER="openai")) == ["OPENAI_API_KEY"]
    assert gp.in_use(voice("kokoro", GHOST_LLM_PROVIDER="ollama")) == []  # no key at all


def test_ghostd_passwords_without_a_daemon(tmp_path, monkeypatch, capsys):
    cfg = cfg_for(tmp_path)
    ring = MemoryKeyring()
    monkeypatch.setattr(gp, "SecretServiceKeyring", lambda: ring)
    monkeypatch.setattr(sys, "stdin", io.StringIO(TOKEN + "\n"))
    assert gp.passwords_cli(["set", "ANTHROPIC_AUTH_TOKEN"], cfg) == 0
    assert gp.passwords_cli(["list"], cfg) == 0
    printed = capsys.readouterr().out
    assert "ANTHROPIC_AUTH_TOKEN: set" in printed and TOKEN not in printed

    (cfg.config_dir / "apps.env").write_text("GHOST_APP_X_KEY=x-key-0123456789\n")
    assert gp.passwords_cli(["import"], cfg) == 0
    printed = capsys.readouterr().out
    assert "imported: GHOST_APP_X_KEY" in printed and "removed" in printed
    assert not (cfg.config_dir / "apps.env").exists()

    assert gp.passwords_cli(["delete", "GHOST_APP_X_KEY"], cfg) == 0
    assert gp.passwords_cli(["bogus"], cfg) == 2
    ring.locked = True
    monkeypatch.setattr(sys, "stdin", io.StringIO("another-value-123\n"))
    assert gp.passwords_cli(["set", "HF_TOKEN"], cfg) == 1
    assert "locked" in capsys.readouterr().err
