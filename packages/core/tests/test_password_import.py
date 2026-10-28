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
    assert any("still holds the password Ghost read heka-vault with" in n for n in report.notes)
    assert heka.exists()  # heka-vault's own file is the operator's to delete


def test_a_password_already_in_the_vault_with_the_same_value_lets_the_files_go(tmp_path):
    config, vault = world(tmp_path)
    vault.set("ANTHROPIC_AUTH_TOKEN", "sk-ant-from-env-file-123")
    report = import_passwords(vault, config)
    assert report.kept == ["ANTHROPIC_AUTH_TOKEN"] and report.differ == []
    assert not (config / "env").exists() and not (config / "apps.env").exists()


def test_a_password_already_in_the_vault_with_another_value_keeps_the_files(tmp_path):
    """The vault's copy wins, but the file's other value may be the only copy of it: the
    files stay, and the note names the password, never either value."""
    config, vault = world(tmp_path)
    vault.set("ANTHROPIC_AUTH_TOKEN", "sk-ant-set-in-ghost-999")
    (config / "settings.env").write_text("GHOST_JEV=auto\nANTHROPIC_AUTH_TOKEN=sk-ant-in-set\n")
    report = import_passwords(vault, config)
    assert report.kept == ["ANTHROPIC_AUTH_TOKEN"]
    assert report.differ == ["ANTHROPIC_AUTH_TOKEN"]
    assert vault.value("ANTHROPIC_AUTH_TOKEN") == "sk-ant-set-in-ghost-999"
    assert report.removed == []
    assert (config / "env").exists() and (config / "apps.env").exists()
    assert read_env(config / "settings.env")["ANTHROPIC_AUTH_TOKEN"] == "sk-ant-in-set"
    assert "MCP_PROXY_KEY" in report.imported  # the rest still goes into the vault
    note = next(n for n in report.notes if "another value" in n)
    assert "ANTHROPIC_AUTH_TOKEN" in note and "Settings › Passwords" in note
    for value in ("sk-ant-set-in-ghost-999", "sk-ant-from-env-file-123", "sk-ant-in-set"):
        assert value not in repr(report)

    # Taken out of the files, the next import removes them.
    (config / "env").write_text("ANTHROPIC_BASE_URL=http://proxy.lan:8317\n")
    (config / "settings.env").write_text("GHOST_JEV=auto\n")
    again = import_passwords(vault, config)
    assert again.differ == [] and not (config / "env").exists()


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


@pytest.mark.parametrize("bad", ["x" * 16_001, "nul\x00inside-0123456"])
def test_a_value_that_cant_be_imported_keeps_the_files(tmp_path, bad):
    """A value the vault refuses may be the only copy: the files holding it stay."""
    config, vault = world(tmp_path)
    (config / "apps.env").write_text(
        'GHOST_APP_MOBBIN_KEY="Bearer mob-0123456789"\n' + f"GHOST_APP_HUGE_KEY={bad}\n"
    )
    report = import_passwords(vault, config)
    assert report.skipped == ["GHOST_APP_HUGE_KEY"]
    assert "GHOST_APP_MOBBIN_KEY" in report.imported
    assert report.removed == []
    assert (config / "env").exists() and (config / "apps.env").exists()
    note = next(n for n in report.notes if "were kept" in n)
    assert "GHOST_APP_HUGE_KEY" in note and "16,000 characters" in note
    assert "take it out of the file" in note  # not "add it on Settings", which refuses it too
    assert bad not in repr(report)

    # A shorter form saved by hand: the file's value is another one, so the files stay
    # until it is taken out of them.
    vault.set("GHOST_APP_HUGE_KEY", "a-good-value-by-hand")
    again = import_passwords(vault, config)
    assert again.skipped == [] and again.differ == ["GHOST_APP_HUGE_KEY"]
    assert (config / "apps.env").exists()
    (config / "apps.env").write_text('GHOST_APP_MOBBIN_KEY="Bearer mob-0123456789"\n')
    third = import_passwords(vault, config)
    assert third.differ == [] and "GHOST_APP_HUGE_KEY" not in third.kept
    assert not (config / "env").exists() and not (config / "apps.env").exists()


def test_a_name_the_vault_cant_take_keeps_the_files(tmp_path):
    """A lowercase name shaped like a password counts as one, but the vault names are
    capitals: it is skipped, and the files stay."""
    config, vault = world(tmp_path)
    (config / "apps.env").write_text("openai_api_key=sk-lower-0123456789\n")
    report = import_passwords(vault, config)
    assert report.skipped == ["openai_api_key"]
    assert (config / "apps.env").exists() and report.removed == []


def test_the_read_back_compares_values_not_names(tmp_path, monkeypatch):
    config, vault = world(tmp_path)
    real = Passwords.holds

    def wrong(self, name, value):  # the vault came back with another value
        return False if name == "MCP_PROXY_KEY" else real(self, name, value)

    monkeypatch.setattr(Passwords, "holds", wrong)
    report = import_passwords(vault, config)
    assert any("didn't read back" in n for n in report.notes)
    assert report.removed == [] and (config / "env").exists()
