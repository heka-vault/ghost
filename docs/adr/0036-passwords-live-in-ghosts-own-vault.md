# 0036: Passwords live in Ghost's own vault, its key in the keyring

- **Status:** accepted (decided 2026-10-28, #31)
- **Date:** 2026-10-28
- **Supersedes:** the rule "secrets come from the environment, loaded from the operator's
  secrets store when the service starts" (CLAUDE.md), and with it `ghost-render-env.sh`;
  ADR 0034's "the key goes into Ghost's environment" and `apps.env`
- **Amends:** ADR 0013 (what "secrets go in and never come out" covers), ADR 0026 (where
  the ElevenLabs key comes from)

Ghost held its secrets as plain text. `~/.config/ghost/env` was rewritten from heka-vault by
`ghost-render-env.sh` at every start and handed to the daemon as its environment; the keys
the desktop saved for apps were in `apps.env` beside it. Both were 0600, but they were plain
files a backup, a sync or a stray `cat` could copy, and every process the daemon started
inherited the lot. The operator also does not think in env vars, API keys and tokens: the
desktop showed "Anthropic token", "OpenRouter key", "Keys for your apps".

## Decisions

**Password is the word.** Every API key, token and account password Ghost holds is a
*password* to the operator, the way an MCP server is an *app*: on the desktop, the phone, in
the voice, the prompts and the operator's docs. Code keeps `secret`.

**One vault, its key in the keyring.** The passwords are one file,
`~/.local/share/ghost/passwords.vault` (0600), sealed with AES-256-GCM (`cryptography`); the
names are sealed too. The vault's key is 32 random bytes kept as one item in the operator's
login keyring through Secret Service (GNOME Keyring, `secretstorage`). It is never on disk.
The keyring is asked only when there is a vault to open or a first password to save. A vault
the keyring's key doesn't open is *broken* and is never overwritten.

**A locked keyring is waited for.** Headless, or before login, the keyring may be locked.
The daemon starts anyway: "Unlock your keyring" is on the waiting list, it tries again every
ten seconds, and the Passwords page offers Unlock (the keyring's own prompt). When the vault
opens, Ghost's settings are read again, Jev joins if it has its password, and the apps that
waited for one connect. A watch's model decider follows at the next start.

**Resolved at the point of use.** The daemon decrypts the vault at start and serves values
from memory. Everywhere else a password is a name (`${GITHUB_TOKEN}`). It becomes its value
only where it is used:

- an app's header, address or arguments when it connects, and a webhook's check: through a
  mapping over the vault that notes each use, for "last used";
- Ghost's own settings (the model connection, Jev, the ElevenLabs voice, Hugging Face's
  download): the values are in the daemon's own process environment, in memory, where those
  settings have always read them;
- **an app that runs on this computer gets the passwords its `env` names as environment
  variables.** That is the one place a value leaves the vault, and only the ones that app
  names.

Nothing else Ghost starts inherits a password: `child_env()` leaves every one out of the
environment of the echo-cancel installer, `notify-send`, the desktop app and the rest.

**Write-only.** A value goes in and never comes back out: not to the desktop, the control
API, a log, the audit, a transcript or the model. `GET /passwords` lists names with whether
each is set, what uses it (Ghost itself, an app, a webhook) and when Ghost last filled it in.
`PUT /passwords/{name}` takes a value as a `SecretStr`, so not even a validation error echoes
it. `ghostd passwords set NAME` reads it without echo.

**Redaction is deterministic.** Every value Ghost holds, at least 8 characters long (shorter
ones would turn page numbers into passwords), is looked for as a plain substring and replaced
with `[password: NAME]` before anything reads the text: an app's results, errors and tool
descriptions; what the store writes (proposals and their audit events, waiting-list items,
transcript turns, compactions); and every log line. A value kept with its scheme
(`Bearer abc…`) is also looked for without it. No model is involved. A change the operator
dictated with a password in it is stored redacted, so a waiting one confirmed later runs with
the placeholder rather than the value.

**heka-vault is a one-time import.** `POST /passwords/import`, the Passwords page's Import
and `ghostd passwords import` (`make passwords-import`) read heka-vault once, when its config
(`~/.config/heka-vault/ghost.env`) is on this computer, then `env` and `apps.env`; put every
password in the vault (one already there is kept); check that the vault reads each value back;
move the non-secret settings `env` held (`ANTHROPIC_BASE_URL`) into `settings.env`; and delete
`env` and `apps.env`, unless a value couldn't be imported (too long, or not one line) or a
file holds another value than the vault's copy of a name: then both stay, and the report names
which (never a value) to take out of the files before importing again. heka-vault's own
config is the operator's to delete. After the import
the vault is the only source; until it, the daemon still reads the two files, redacts their
values and keeps them from child processes, and the waiting list asks for the import.

**No plaintext secret files.** The service, the deploy drop-in, `make dev`, `make daemon`
and the make targets load `settings.env`, which holds only non-secret settings, and the
daemon opens the vault itself. `ghost-render-env.sh` is gone.

## Consequences

- `ghost_core.passwords` (the vault, the keyring, redaction, `child_env`) and
  `ghost_core.password_import` are new; `ghost_core.envfile` has the KEY=value helpers.
  `ghostd.passwords` opens the vault for every command that needs it.
- The control API: `/passwords` (list, set, delete, import, unlock). `SettingsView` keeps
  `secrets` as set-or-not, from the vault, and loses `env_path` and `vault_managed`.
- Settings › Passwords is the vault. Each password saves on its own row; one Ghost reads
  itself joins the restart pill. Connecting an app with a key says password and saves it in
  the vault, refused while the keyring is locked.
- Tests use a fake keyring and temp folders; nothing touches the operator's keyring.
- What stays plain: sign-in tokens (`~/.local/share/ghost/apps/<name>/oauth.json`) and the
  phone's pairing secret (`phone.json`), both 0600 and written by Ghost, not the operator.
  Moving them into the vault is a later step.

## Considered options

- **Keep the env files, tighten their modes.** Rejected: they are already 0600; the problem
  is plain text on disk and an environment every child inherits.
- **heka-vault at every start, as before.** Rejected by the operator: two sources of truth,
  and a network fetch before Ghost can start.
- **The `keyring` package for the values themselves.** Rejected: one keyring item per
  password, no metadata, and Secret Service is slow per item. The keyring holds one key.
- **A password-protected file, no keyring.** Rejected: a password to type at every start of
  a service that starts at login.
- **Keep values out of the process environment entirely.** Rejected for now: Ghost's own
  settings and Hugging Face's library read the environment; resolving each at its point of
  use means changes inside the voice pipeline. The environment is in memory only, and
  `child_env` keeps it from every child.
