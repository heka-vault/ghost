"""The one-time import (ADR 0036): heka-vault and the plaintext env files into the vault,
read back, then the plaintext copies removed. Temp folders and a fake keyring and a fake
heka-vault only."""

from __future__ import annotations

from pathlib import Path

import pytest
from ghost_core import passwords as pw
from ghost_core.envfile import read_env
from ghost_core.password_import import import_passwords, legacy_values, sources
from ghost_core.passwords import MemoryKeyring, Passwords, VaultClosed


@pytest.fixture(autouse=True)
def _clean_redactor():
    yield
    pw._REDACTOR._sets.clear()
    pw._REDACTOR._pairs = ()


def world(tmp_path: Path):
    config = tmp_path / "config"
    config.mkdir()
    (config / "env").write_text(
        "ANTHROPIC_AUTH_TOKEN=sk-ant-from-env-file-123\n"
        "ANTHROPIC_BASE_URL=http://proxy.lan:8317\n"
        "GHOST_INPUT_DEVICE=ghost.mic\n"
        "MCP_PROXY_KEY=proxy-key-0123456789\n"
    )
    (config / "apps.env").write_text('GHOST_APP_MOBBIN_KEY="Bearer mob-0123456789"\n')
    (config / "settings.env").write_text("GHOST_INPUT_DEVICE=ghost.mic\nGHOST_JEV=auto\n")
    ring = MemoryKeyring()
    vault = Passwords(tmp_path / "data" / "passwords.vault", ring)
    assert vault.open()
    return config, vault


def test_env_files_move_into_the_vault_and_are_removed(tmp_path):
    config, vault = world(tmp_path)
    assert {s.path.name for s in sources(config, None)} == {"env", "apps.env"}
    report = import_passwords(vault, config)
    assert report.imported == ["ANTHROPIC_AUTH_TOKEN", "GHOST_APP_MOBBIN_KEY", "MCP_PROXY_KEY"]
    assert report.settings == ["ANTHROPIC_BASE_URL"]
    assert {p.name for p in report.removed} == {"env", "apps.env"}
    assert not (config / "env").exists() and not (config / "apps.env").exists()
    assert read_env(config / "settings.env") == {
        "GHOST_INPUT_DEVICE": "ghost.mic",
        "GHOST_JEV": "auto",
        "ANTHROPIC_BASE_URL": "http://proxy.lan:8317",
    }
    fresh = Passwords(vault.path, vault.keyring)
    assert fresh.open()
    assert fresh.value("GHOST_APP_MOBBIN_KEY") == "Bearer mob-0123456789"
    assert sources(config, None) == []
    # Nothing in the report is a value.
    assert "sk-ant-from-env-file-123" not in repr(report)


def test_heka_vault_is_read_once_and_the_files_win(tmp_path):
    config, vault = world(tmp_path)
    heka = tmp_path / "heka" / "ghost.env"
    heka.parent.mkdir()
    heka.write_text(
        "HEKA_VAULT_URL=https://vault.lan\nHEKA_VAULT_PROJECT=ghost\nHEKA_VAULT_TOKEN=t\n"
    )
    held = {
        "ANTHROPIC_AUTH_TOKEN": "sk-ant-from-heka-old",
        "HF_TOKEN": "hf_from_heka_0123456",
        "CLIPROXY_BASE_URL": "http://cliproxy.lan:8317",
        "LINEAR_TOKEN": "lin_from_heka_01234",
    }
    asked: list[str] = []

    def fetch(url, project, token, name):
        assert (url, project, token) == ("https://vault.lan", "ghost", "t")
        asked.append(name)
        if name == "ELEVENLABS_API_KEY":
            raise ConnectionError("down")
        return held.get(name)

    report = import_passwords(
        vault, config, heka_env=heka, referenced={"LINEAR_TOKEN"}, fetch=fetch
    )
    assert "LINEAR_TOKEN" in asked
    assert set(report.imported) == {
        "ANTHROPIC_AUTH_TOKEN",
        "HF_TOKEN",
        "LINEAR_TOKEN",
        "GHOST_APP_MOBBIN_KEY",
        "MCP_PROXY_KEY",
    }
    assert vault.value("ANTHROPIC_AUTH_TOKEN") == "sk-ant-from-env-file-123"  # newer copy
    # The env file's base URL is explicit, so CLIPROXY's isn't used.
    assert read_env(config / "settings.env")["ANTHROPIC_BASE_URL"] == "http://proxy.lan:8317"
    assert any("heka-vault didn't answer for 1" in n for n in report.notes)
    assert any("still holds heka-vault's read token" in n for n in report.notes)
    assert heka.exists()  # heka-vault's own file is the operator's to delete


def test_a_password_already_in_the_vault_is_kept(tmp_path):
    config, vault = world(tmp_path)
    vault.set("ANTHROPIC_AUTH_TOKEN", "sk-ant-set-in-ghost-999")
    report = import_passwords(vault, config)
    assert report.kept == ["ANTHROPIC_AUTH_TOKEN"]
    assert vault.value("ANTHROPIC_AUTH_TOKEN") == "sk-ant-set-in-ghost-999"


def test_a_password_written_into_settings_env_leaves_it(tmp_path):
    config, vault = world(tmp_path)
    (config / "settings.env").write_text("GHOST_JEV=auto\nTYPESAFE_API_KEY=ts_0123456789\n")
    report = import_passwords(vault, config)
    assert "TYPESAFE_API_KEY" in report.imported
    assert "TYPESAFE_API_KEY" not in read_env(config / "settings.env")


def test_a_locked_vault_touches_nothing(tmp_path):
    config, _ = world(tmp_path)
    ring = MemoryKeyring(locked=True)
    vault = Passwords(tmp_path / "data" / "passwords.vault", ring)
    assert vault.open()  # no file yet: open, but saving needs the keyring
    with pytest.raises(VaultClosed):
        import_passwords(vault, config)
    assert (config / "env").exists() and (config / "apps.env").exists()


def test_nothing_to_import(tmp_path):
    config = tmp_path / "config"
    config.mkdir()
    vault = Passwords(tmp_path / "passwords.vault", MemoryKeyring())
    assert vault.open()
    report = import_passwords(vault, config)
    assert report.notes == ["There was nothing to import."]


def test_legacy_values_read_both_files(tmp_path):
    config, _ = world(tmp_path)
    values = legacy_values(config)
    assert values["MCP_PROXY_KEY"] == "proxy-key-0123456789"
    assert values["GHOST_APP_MOBBIN_KEY"] == "Bearer mob-0123456789"
