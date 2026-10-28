"""Moving passwords into Ghost's vault, once (ADR 0036).

Before the vault, Ghost's passwords were plaintext: ``~/.config/ghost/env``, which
``ghost-render-env.sh`` rewrote from heka-vault at every start, and ``apps.env``, the keys
the desktop kept for apps. The import takes the values from heka-vault (when its config,
``~/.config/heka-vault/ghost.env``, is on this computer), then from those two files, puts
them in the vault, checks the vault reads each value back, and removes the plaintext files,
unless a value couldn't be imported (too long, or not one line) or a file holds another
value than the vault for a name already in it: then the files stay. The non-secret settings
in ``env`` (``ANTHROPIC_BASE_URL``, and anything the renderer copied) move to
``settings.env``, which is plain text. A password already in the vault is kept.

Nothing here prints, logs or returns a value: the report names the passwords.
"""

from __future__ import annotations

from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from pathlib import Path

from loguru import logger

from ghost_core.envfile import read_env, update_env
from ghost_core.passwords import (
    MAX_VALUE_CHARS,
    Passwords,
    VaultClosed,
    check_name,
    check_value,
    is_password_name,
)

LEGACY_ENV = "env"  # ~/.config/ghost/env
LEGACY_APPS_ENV = "apps.env"  # ~/.config/ghost/apps.env
SETTINGS_ENV = "settings.env"
# What ghost-render-env.sh fetched from heka-vault.
HEKA_KEYS = (
    "ANTHROPIC_BASE_URL",
    "ANTHROPIC_AUTH_TOKEN",
    "ANTHROPIC_API_KEY",  # the model connection's own keys (ADR 0037)
    "OPENAI_API_KEY",
    "CLIPROXY_BASE_URL",
    "GITHUB_TOKEN",
    "HF_TOKEN",
    "TYPESAFE_API_KEY",
    "OPENROUTER_API_KEY",
    "ELEVENLABS_API_KEY",
)
# Settings heka-vault held that are not passwords; CLIPROXY_BASE_URL was only ever the
# fallback for ANTHROPIC_BASE_URL.
HEKA_SETTINGS = ("ANTHROPIC_BASE_URL", "CLIPROXY_BASE_URL")
FETCH_TIMEOUT_SECS = 10.0

# (url, project, token, name) -> the value, or None when heka-vault has none
Fetch = Callable[[str, str, str, str], str | None]


@dataclass
class ImportReport:
    imported: list[str] = field(default_factory=list)  # new in the vault
    kept: list[str] = field(default_factory=list)  # already in the vault; left as it was
    settings: list[str] = field(default_factory=list)  # moved to settings.env
    removed: list[Path] = field(default_factory=list)  # plaintext files deleted
    skipped: list[str] = field(default_factory=list)  # not imported; their files were kept
    # kept, but a plaintext file holds another value: the files were kept
    differ: list[str] = field(default_factory=list)
    notes: list[str] = field(default_factory=list)  # what else to know; never a value


@dataclass(frozen=True)
class ImportSource:
    kind: str  # heka_vault | file
    path: Path


def sources(config_dir: Path, heka_env: Path | None) -> list[ImportSource]:
    """What there is to import on this computer."""
    out = []
    if heka_env is not None and heka_env.is_file():
        out.append(ImportSource("heka_vault", heka_env))
    for name in (LEGACY_ENV, LEGACY_APPS_ENV):
        p = config_dir / name
        if p.is_file():
            out.append(ImportSource("file", p))
    settings = config_dir / SETTINGS_ENV
    if settings.is_file() and any(is_password_name(k) for k in read_env(settings)):
        out.append(ImportSource("file", settings))
    return out


def legacy_values(config_dir: Path) -> dict[str, str]:
    """What the plaintext files hold, for a daemon that runs before the import: ``env``,
    then ``apps.env``. The caller keeps them out of logs and child processes."""
    out: dict[str, str] = {}
    for name in (LEGACY_ENV, LEGACY_APPS_ENV):
        out.update(read_env(config_dir / name))
    return out


def fetch_heka(url: str, project: str, token: str, name: str) -> str | None:
    import httpx

    r = httpx.get(
        f"{url.rstrip('/')}/api/v1/projects/{project}/secrets/{name}",
        params={"env": "dev", "format": "raw"},
        headers={"Authorization": f"Bearer {token}"},
        timeout=FETCH_TIMEOUT_SECS,
    )
    if r.status_code == 404:
        return None
    r.raise_for_status()
    return r.text


def import_passwords(
    vault: Passwords,
    config_dir: Path,
    *,
    heka_env: Path | None = None,
    referenced: Iterable[str] = (),
    fetch: Fetch = fetch_heka,
) -> ImportReport:
    """Move every password on this computer into ``vault`` and remove the plaintext files.
    ``referenced``: the names the connected apps and the webhooks use, so they count as
    passwords whatever they're called. Raises ``VaultClosed`` before touching anything
    when the vault can't be written."""
    if not vault.is_open:
        raise VaultClosed(vault.problem or "the vault isn't open")
    report = ImportReport()
    named = set(referenced)
    settings_path = config_dir / SETTINGS_ENV
    settings = read_env(settings_path)
    for listed in ("GHOST_APP_SECRETS", "GHOST_MCP_SECRETS"):
        named |= {n.strip() for n in (settings.get(listed) or "").split(",") if n.strip()}

    found: dict[str, str] = {}  # password -> value, later sources win
    moved: dict[str, str] = {}  # non-secret setting -> value, for settings.env

    if heka_env is not None and heka_env.is_file():
        _from_heka(heka_env, named, fetch, found, moved, report)

    env_path = config_dir / LEGACY_ENV
    in_files: dict[str, set[str]] = {}  # name -> every value a plaintext file holds for it
    for k, v in read_env(env_path).items():
        if is_password_name(k, named):
            found[k] = v
            in_files.setdefault(k, set()).add(v.strip())
        elif k not in settings:
            moved[k] = v
    for k, v in read_env(config_dir / LEGACY_APPS_ENV).items():
        found[k] = v
        in_files.setdefault(k, set()).add(v.strip())
    in_settings = [k for k in settings if is_password_name(k, named)]
    for k in in_settings:
        found[k] = settings[k]
        in_files.setdefault(k, set()).add(settings[k].strip())

    new: dict[str, str] = {}
    for name in sorted(found):
        value = found[name].strip()
        if name in vault:  # the vault's copy wins, even over one that can't be imported
            report.kept.append(name)
            # A file holding another value may hold the only copy of it: compared, never
            # shown.
            if any(v and not vault.holds(name, v) for v in in_files.get(name, ())):
                report.differ.append(name)
            continue
        if not value:
            continue  # nothing to lose
        try:
            check_name(name)
            check_value(value)
        except ValueError as e:
            report.skipped.append(name)
            report.notes.append(f"{name} wasn't imported: {e}")
            continue
        new[name] = value
    if new:
        vault.set_many(new)
    report.imported = sorted(new)

    # The vault must read back every value before a plaintext copy goes: the same value,
    # not only the name.
    check = Passwords(vault.path, vault.keyring)
    if (
        not check.open()
        or not all(check.holds(n, v) for n, v in new.items())
        or not all(n in check for n in report.kept)
    ):
        report.notes.append(
            "the vault didn't read back; the plaintext files were left as they were"
        )
        return report

    if "ANTHROPIC_BASE_URL" not in settings and "ANTHROPIC_BASE_URL" not in moved:
        if "CLIPROXY_BASE_URL" in moved:
            moved["ANTHROPIC_BASE_URL"] = moved["CLIPROXY_BASE_URL"]
    moved.pop("CLIPROXY_BASE_URL", None)
    changes: dict[str, str | None] = {k: v for k, v in moved.items() if k not in settings}
    # A password written into settings.env leaves it, once it is in the vault.
    stay = set(report.skipped) | set(report.differ)
    changes.update(dict.fromkeys(k for k in in_settings if k not in stay))
    if changes:
        update_env(settings_path, changes, mode=0o600)
    report.settings = sorted(k for k, v in changes.items() if v is not None)

    plain = [p for p in (env_path, config_dir / LEGACY_APPS_ENV) if p.exists()]
    files = " and ".join(_tilde(p) for p in plain) or _tilde(settings_path)
    were = "were" if len(plain) > 1 else "was"
    if report.skipped:
        # A skipped value may be the only copy: no file that may hold it is deleted. The
        # vault takes no more than Settings › Passwords does, so that is no way in either.
        many = len(report.skipped) > 1
        report.notes.append(
            f"{files} {were} kept, since {', '.join(report.skipped)}"
            f" {'are' if many else 'is'} still only there, and Ghost can't keep"
            f" {'them' if many else 'it'}: a password is one line of at most"
            f" {MAX_VALUE_CHARS:,} characters, named in capital letters, digits and _."
            " If what uses it takes a shorter form, save that on Settings › Passwords;"
            " otherwise copy it somewhere safe. Then take it out of the file and import"
            " again to remove the plain files."
        )
    if report.differ:
        # The vault's copy is kept and used; the file's may be the only copy of another.
        many = len(report.differ) > 1
        report.notes.append(
            f"{files} {were} kept, since {', '.join(report.differ)}"
            f" {'have' if many else 'has'} another value there than in Ghost's passwords."
            " Ghost uses the one in its passwords. If the file's is the right one, save it"
            " on Settings › Passwords. Then take it out of the file and import again to"
            " remove the plain files."
        )
    if report.skipped or report.differ:
        plain = []
    for path in plain:
        path.unlink()
        report.removed.append(path)
    if heka_env is not None and heka_env.is_file():
        report.notes.append(
            f"{_tilde(heka_env)} still holds the password Ghost read heka-vault with. Ghost"
            " doesn't read it any more; delete it when nothing else needs it."
        )
    if not (report.imported or report.kept or report.removed or report.settings):
        report.notes.append("There was nothing to import.")
    logger.info(
        f"passwords: imported {len(report.imported)}, kept {len(report.kept)}, "
        f"removed {len(report.removed)} plaintext file(s)"
    )
    return report


def _from_heka(
    heka_env: Path,
    named: set[str],
    fetch: Fetch,
    found: dict[str, str],
    moved: dict[str, str],
    report: ImportReport,
) -> None:
    conf = read_env(heka_env)
    url, project, token = (
        conf.get(k, "") for k in ("HEKA_VAULT_URL", "HEKA_VAULT_PROJECT", "HEKA_VAULT_TOKEN")
    )
    if not (url and project and token):
        report.notes.append(
            f"{_tilde(heka_env)} doesn't name heka-vault's address, project and token"
        )
        return
    failed = 0
    for name in dict.fromkeys([*HEKA_KEYS, *sorted(named)]):
        try:
            value = fetch(url, project, token, name)
        except Exception as e:  # noqa: BLE001 - the store's own trouble; never its body
            failed += 1
            logger.warning(f"passwords: heka-vault didn't give {name} ({type(e).__name__})")
            continue
        if value is None:
            continue
        if name in HEKA_SETTINGS:
            moved[name] = value.strip()
        else:
            found[name] = value
    if failed:
        report.notes.append(f"heka-vault didn't answer for {failed} name(s); see the log")


def _tilde(p: Path) -> str:
    home = str(Path.home())
    s = str(p)
    return "~" + s[len(home) :] if s.startswith(home) else s


__all__ = [
    "HEKA_KEYS",
    "ImportReport",
    "ImportSource",
    "fetch_heka",
    "import_passwords",
    "legacy_values",
    "sources",
]
