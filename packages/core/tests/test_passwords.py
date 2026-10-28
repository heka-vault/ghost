"""The vault (ADR 0036): sealed on disk, its key in the keyring, values only at the point of
use, and every value redacted from what comes back. A fake keyring throughout: these tests
never touch the operator's keyring or files."""

from __future__ import annotations

import json
import os
import stat
from pathlib import Path

import pytest
from ghost_core import passwords as pw
from ghost_core.passwords import (
    MemoryKeyring,
    Passwords,
    VaultClosed,
    child_env,
    is_password_name,
    redact,
    redact_text,
)

TOKEN = "ghp_0123456789abcdefABCDEF"
OTHER = "sk-ant-api03-zyxwvutsrqponm"


@pytest.fixture(autouse=True)
def _clean_redactor():
    pw._REDACTOR._sets.clear()
    pw._REDACTOR._pairs = ()
    yield
    pw._REDACTOR._sets.clear()
    pw._REDACTOR._pairs = ()


def vault(tmp_path: Path, keyring: MemoryKeyring | None = None, environ=None) -> Passwords:
    v = Passwords(tmp_path / "passwords.vault", keyring or MemoryKeyring(), environ=environ)
    assert v.open()
    return v


def test_a_new_vault_is_open_and_needs_the_keyring_only_to_save(tmp_path):
    ring = MemoryKeyring(locked=True)
    v = Passwords(tmp_path / "passwords.vault", ring)
    assert v.open() and v.is_open and len(v) == 0
    with pytest.raises(VaultClosed):
        v.set("GITHUB_TOKEN", TOKEN)
    assert v.state == "locked"
    assert not (tmp_path / "passwords.vault").exists()


def test_saved_sealed_and_read_back(tmp_path):
    ring = MemoryKeyring()
    v = vault(tmp_path, ring)
    v.set("GITHUB_TOKEN", TOKEN)
    path = tmp_path / "passwords.vault"
    raw = path.read_bytes()
    assert TOKEN.encode() not in raw and b"GITHUB_TOKEN" not in raw  # names are sealed too
    assert json.loads(raw)["cipher"] == "AES-256-GCM"
    assert stat.S_IMODE(os.stat(path).st_mode) == 0o600
    assert ring.key is not None and len(ring.key) == 32

    again = Passwords(path, ring)
    assert again.open()
    assert again.names() == ["GITHUB_TOKEN"]
    assert again.value("GITHUB_TOKEN") == TOKEN


def test_a_locked_keyring_waits_and_a_wrong_key_never_overwrites(tmp_path):
    ring = MemoryKeyring()
    vault(tmp_path, ring).set("GITHUB_TOKEN", TOKEN)
    path = tmp_path / "passwords.vault"
    before = path.read_bytes()

    ring.locked = True
    locked = Passwords(path, ring)
    assert not locked.open()
    assert locked.state == "locked" and locked.problem == "Your keyring is locked"
    assert locked.names() == []
    ring.locked = False
    assert locked.open() and locked.names() == ["GITHUB_TOKEN"]

    wrong = Passwords(path, MemoryKeyring(b"\x01" * 32))
    assert not wrong.open() and wrong.state == "broken"
    with pytest.raises(VaultClosed):
        wrong.set("X_TOKEN", "a-value-long-enough")
    gone = Passwords(path, MemoryKeyring(None))  # the keyring lost the key
    assert not gone.open() and gone.state == "broken"
    assert path.read_bytes() == before

    absent = Passwords(path, MemoryKeyring(available=False))
    assert not absent.open() and absent.state == "unavailable"


def test_unlock_asks_the_keyring_then_opens(tmp_path):
    ring = MemoryKeyring()
    vault(tmp_path, ring).set("GITHUB_TOKEN", TOKEN)
    ring.locked = True
    v = Passwords(tmp_path / "passwords.vault", ring)
    assert not v.open()
    assert v.unlock() and v.is_open


def test_names_and_metadata_never_the_value(tmp_path):
    v = vault(tmp_path)
    v.set("GITHUB_TOKEN", TOKEN)
    (meta,) = v.info()
    assert meta.name == "GITHUB_TOKEN" and meta.last_used is None
    assert TOKEN not in repr(meta) and TOKEN not in repr(v)
    assert TOKEN not in repr(v._entries["GITHUB_TOKEN"])


def test_replace_delete_and_the_environment_kept_in_step(tmp_path):
    env: dict[str, str] = {}
    v = vault(tmp_path, environ=env)
    v.set("GITHUB_TOKEN", TOKEN)
    assert env == {"GITHUB_TOKEN": TOKEN}
    v.set("GITHUB_TOKEN", OTHER)
    assert env["GITHUB_TOKEN"] == OTHER
    assert v.delete("GITHUB_TOKEN", "NOPE") == ["GITHUB_TOKEN"]
    assert env == {} and v.names() == []


@pytest.mark.parametrize(
    "name,value",
    [("lower", TOKEN), ("1ABC", TOKEN), ("A", ""), ("A", "  "), ("A", "two\nlines")],
)
def test_bad_names_and_values_are_refused(tmp_path, name, value):
    with pytest.raises(ValueError):
        vault(tmp_path).set(name, value)


def test_a_write_from_another_process_is_not_lost(tmp_path):
    ring = MemoryKeyring()
    daemon = vault(tmp_path, ring)
    daemon.set("GITHUB_TOKEN", TOKEN)
    cli = vault(tmp_path, ring)
    cli.set("HF_TOKEN", "hf_abcdefghijklmnop")
    daemon.set("OPENROUTER_API_KEY", OTHER)
    assert Passwords(tmp_path / "passwords.vault", ring).open()
    assert daemon.names() == ["GITHUB_TOKEN", "HF_TOKEN", "OPENROUTER_API_KEY"]


def test_use_is_noted_through_the_environ_mapping(tmp_path):
    v = vault(tmp_path)
    v.set("GITHUB_TOKEN", TOKEN)
    env = v.environ({"PATH": "/bin"})
    assert "GITHUB_TOKEN" in env and "PATH" in env and "NOPE" not in env
    assert v.info()[0].last_used is None  # asking isn't using
    from ghost_core.apps import fill_secrets

    assert fill_secrets("Bearer ${GITHUB_TOKEN}", env) == f"Bearer {TOKEN}"
    assert v.info()[0].last_used is not None
    reread = Passwords(tmp_path / "passwords.vault", v.keyring)
    assert reread.open() and reread.info()[0].last_used is not None  # written back
    with pytest.raises(KeyError):
        fill_secrets("${NOPE}", env)


def test_redaction_replaces_every_value_in_nested_results(tmp_path):
    v = vault(tmp_path)
    v.set_many({"GITHUB_TOKEN": TOKEN, "APP_KEY": f"Bearer {OTHER}", "PIN": "1234"})
    result = {
        "text": f"your token is {TOKEN}.",
        "items": [{"auth": OTHER}, (TOKEN,)],
        TOKEN: "as a key",
        "n": 3,
    }
    out = redact(result)
    assert out["text"] == "your token is [password: GITHUB_TOKEN]."
    assert out["items"][0]["auth"] == "[password: APP_KEY]"  # the token inside "Bearer …"
    assert out["items"][1] == ("[password: GITHUB_TOKEN]",)
    assert out["[password: GITHUB_TOKEN]"] == "as a key" and out["n"] == 3
    # A value shorter than the minimum isn't looked for: page 1234 stays page 1234.
    assert redact_text("page 1234") == "page 1234"
    assert TOKEN not in json.dumps(out)


def test_a_value_inside_another_is_replaced_whole(tmp_path):
    v = vault(tmp_path)
    v.set_many({"SHORT_KEY": "abcdefgh12", "LONG_KEY": "abcdefgh12345678"})
    assert redact_text("x abcdefgh12345678 y abcdefgh12") == (
        "x [password: LONG_KEY] y [password: SHORT_KEY]"
    )


def test_deleted_passwords_stop_being_redacted(tmp_path):
    v = vault(tmp_path)
    v.set("GITHUB_TOKEN", TOKEN)
    v.delete("GITHUB_TOKEN")
    assert redact_text(TOKEN) == TOKEN


def test_child_env_leaves_every_password_out(tmp_path):
    v = vault(tmp_path)
    v.set("GITHUB_TOKEN", TOKEN)
    pw.also_redact("legacy", {"OLD_TOKEN": "legacy-value-1234"})
    env = child_env({"PATH": "/bin", "GITHUB_TOKEN": TOKEN, "OLD_TOKEN": "x", "HOME": "/h"})
    assert env == {"PATH": "/bin", "HOME": "/h"}
    assert redact_text("legacy-value-1234") == "[password: OLD_TOKEN]"


@pytest.mark.parametrize(
    "name,expected",
    [
        ("ANTHROPIC_AUTH_TOKEN", True),
        ("GHOST_APP_MOBBIN_KEY", True),
        ("MCP_PROXY_KEY", True),
        ("SOME_PASSWORD", True),
        ("ANTHROPIC_BASE_URL", False),
        ("GHOST_LLM_MODEL", False),
        ("GHOST_JEV", False),
        ("CUSTOM", False),
    ],
)
def test_which_names_are_passwords(name, expected):
    assert is_password_name(name) is expected
    assert is_password_name("CUSTOM", ["CUSTOM"])
