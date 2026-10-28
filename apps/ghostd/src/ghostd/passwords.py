"""The daemon's side of Ghost's vault (ADR 0036): opening it at start, the plaintext files
a machine still has before its import, what the waiting list says, and
``ghostd passwords``.

A process that needs passwords (``ghostd run``, ``check``, ``perf``, ``push``) opens the
vault itself; nothing hands it a secret through its environment any more. The values go
into this process's own environment, in memory, where Ghost's settings read them, and into
the redactor; ``ghost_core.passwords.child_env`` keeps them from anything it starts.
"""

from __future__ import annotations

import getpass
import os
import sys
from pathlib import Path
from typing import Any

from ghost_core.password_import import import_passwords, legacy_values, sources
from ghost_core.passwords import (
    GHOST_USES,
    VAULT_FILE,
    Passwords,
    SecretServiceKeyring,
    VaultClosed,
    also_redact,
    is_password_name,
    redact_text,
)
from loguru import logger

VAULT_RETRY_SECS = 10.0  # a locked keyring is tried again this often


def vault_path(cfg: Any) -> Path:
    return Path(cfg.data_dir) / VAULT_FILE


def open_passwords(cfg: Any, *, keyring: Any = None, environ: Any = None) -> Passwords:
    """The vault, opened if it can be, its values in this process's environment. A machine
    that hasn't imported yet still has ``~/.config/ghost/env`` and ``apps.env``: their
    values are read too, under the vault's (which win), and kept out of logs and child
    processes like the vault's, until the import removes the files."""
    env = os.environ if environ is None else environ
    legacy = legacy_values(Path(cfg.config_dir))
    if legacy:
        for k, v in legacy.items():
            env.setdefault(k, v)
        also_redact("legacy", {k: v for k, v in legacy.items() if is_password_name(k)})
    vault = Passwords(vault_path(cfg), keyring or SecretServiceKeyring(), environ=env)
    vault.open()
    return vault


def problems(vault: Passwords | None, cfg: Any) -> list[tuple[str, str]]:
    """Ghost's own items about its passwords, for the waiting list (ADR 0029)."""
    out: list[tuple[str, str]] = []
    if vault is not None and vault.state == "locked":
        out.append(("passwords:locked", "Unlock your keyring so Ghost can read its passwords"))
    elif vault is not None and not vault.is_open:
        out.append(("passwords:vault", f"Ghost can't open its passwords: {vault.problem}"))
    if any(s.kind == "file" for s in sources(Path(cfg.config_dir), None)):
        out.append(
            (
                "passwords:import",
                "Some passwords are still in plain files. Import them on Settings › Passwords",
            )
        )
    return out


def in_use(voice: Any) -> list[str]:
    """Ghost's own passwords a session fills in, for "last used"."""
    names = [
        "OPENROUTER_API_KEY"
        if getattr(voice, "llm_endpoint", "proxy") == "openrouter"
        else "ANTHROPIC_AUTH_TOKEN"
    ]
    if getattr(voice, "tts_engine", "kokoro") == "elevenlabs":
        names.append("ELEVENLABS_API_KEY")
    return names


def redacting(record: dict[str, Any]) -> None:
    """A loguru patcher: no log line carries a password (ADR 0036)."""
    record["message"] = redact_text(record["message"])


# --- ghostd passwords ------------------------------------------------------------------

USAGE = """usage: ghostd passwords [list | set NAME | delete NAME | import | unlock]

  list           what is in Ghost's vault (names only; a value is never shown)
  set NAME       save one; the value is read from the terminal without echo, or stdin
  delete NAME    forget one
  import         once: heka-vault and ~/.config/ghost/env and apps.env into the vault,
                 then the plaintext files are removed
  unlock         ask your keyring to unlock, then open the vault

With Ghost running, these go through it; otherwise they edit the vault directly."""


def passwords_cli(args: list[str], cfg: Any) -> int:
    cmd = args[0] if args else "list"
    if cmd not in {"list", "set", "delete", "import", "unlock"} or (
        cmd in {"set", "delete"} and len(args) != 2
    ):
        print(USAGE, file=sys.stderr)
        return 2
    value = _read_value(args[1]) if cmd == "set" else None
    if cmd == "set" and not value:
        print("no value given; nothing saved", file=sys.stderr)
        return 2
    from ghost_api import GhostClient, default_socket_path

    sock = cfg.api_socket or default_socket_path()
    if sock.exists():
        client = GhostClient(sock)
        try:
            return _through_daemon(client, cmd, args, value)
        finally:
            client.close()
    return _direct(cfg, cmd, args, value)


def _read_value(name: str) -> str:
    if sys.stdin.isatty():
        return getpass.getpass(f"{name}: ").strip()
    return sys.stdin.readline().strip()


def _print_list(view: dict[str, Any]) -> None:
    vault = view["vault"]
    why = f": {vault['problem']}" if vault["problem"] else ""
    print(f"vault: {vault['path']} ({vault['state']}{why})")
    for p in view["passwords"]:
        mark = "set" if p["set"] else "not set"
        uses = ", ".join(p["used_by"]) or "nothing names it"
        last = f", last used {p['last_used'][:16]}" if p.get("last_used") else ""
        print(f"  {p['name']}: {mark} ({uses}{last})")
    for s in view.get("importable") or []:
        print(f"  to import: {s['path']}")


def _print_report(out: dict[str, Any]) -> None:
    print(f"imported: {', '.join(out['imported']) or 'none'}")
    if out["kept"]:
        print(f"already in the vault, kept: {', '.join(out['kept'])}")
    if out["settings"]:
        print(f"moved to settings.env: {', '.join(out['settings'])}")
    for path in out["removed"]:
        print(f"removed {path}")
    for note in out["notes"]:
        print(note)
    if out.get("restart_required"):
        print("Restart Ghost so it uses them: make service-on, or make reload under make dev.")


def _through_daemon(client: Any, cmd: str, args: list[str], value: str | None) -> int:
    if cmd == "list":
        _print_list(client.passwords())
        return 0
    if cmd == "set":
        r = client.set_password(args[1], value)
    elif cmd == "delete":
        r = client.delete_password(args[1])
    elif cmd == "unlock":
        r = client.unlock_passwords()
    else:
        r = client.import_passwords()
    if r.status_code >= 400:
        print(r.json().get("detail", r.text), file=sys.stderr)
        return 1
    out = r.json()
    if cmd == "import":
        _print_report(out)
    elif cmd == "unlock":
        print(f"vault: {out['vault']['state']}")
        return 0 if out["vault"]["state"] == "open" else 1
    else:
        print(f"{args[1]}: {'saved' if cmd == 'set' else 'forgotten'}")
        if out.get("restart_required"):
            print("Ghost uses it from its next start.")
    return 0


def _direct(cfg: Any, cmd: str, args: list[str], value: str | None) -> int:
    """Ghost isn't running: the vault file itself. The daemon reads it at its next start."""
    vault = Passwords(vault_path(cfg), SecretServiceKeyring())
    opened = vault.unlock() if cmd == "unlock" else vault.open()
    if cmd == "unlock":
        print(f"vault: {vault.state}")
        return 0 if opened else 1
    try:
        if cmd == "list":
            from ghost_api.passwords import tilde

            rows = [
                {
                    "name": m.name,
                    "set": True,
                    "used_by": [GHOST_USES[m.name]] if m.name in GHOST_USES else [],
                    "last_used": m.last_used.isoformat() if m.last_used else None,
                }
                for m in vault.info()
            ]
            view = {
                "vault": {
                    "path": tilde(vault.path),
                    "state": vault.state,
                    "problem": vault.problem,
                },
                "passwords": rows,
                "importable": [
                    {"path": tilde(s.path)}
                    for s in sources(Path(cfg.config_dir), Path(cfg.vault_env_path))
                ],
            }
            _print_list(view)
            return 0
        if not opened:
            print(f"Ghost's vault isn't open: {vault.problem}", file=sys.stderr)
            return 1
        if cmd == "set":
            vault.set(args[1], value or "")
            print(f"{args[1]}: saved")
        elif cmd == "delete":
            print(f"{args[1]}: {'forgotten' if vault.delete(args[1]) else 'not in the vault'}")
        else:
            from ghost_api.passwords import tilde
            from ghost_core.apps import APPS_FILE, load_apps
            from ghost_core.folder import GhostFolder

            names = set(GHOST_USES)
            for spec in load_apps(Path(cfg.config_dir) / APPS_FILE).apps.values():
                names.update(spec.secrets())
            names.update(h.secret_name() for h in GhostFolder(Path(cfg.config_dir)).hooks.values())
            report = import_passwords(
                vault,
                Path(cfg.config_dir),
                heka_env=Path(cfg.vault_env_path),
                referenced=names,
            )
            _print_report(
                {
                    "imported": report.imported,
                    "kept": report.kept,
                    "settings": report.settings,
                    "removed": [tilde(p) for p in report.removed],
                    "notes": report.notes,
                    "restart_required": False,
                }
            )
    except (ValueError, VaultClosed) as e:
        print(str(e), file=sys.stderr)
        return 1
    logger.debug(f"passwords: {cmd} done")
    return 0
