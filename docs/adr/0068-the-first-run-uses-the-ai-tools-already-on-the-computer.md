# 0068: The first run uses the AI tools already on the computer

- **Status:** accepted
- **Date:** 2026-11-14
- **Builds on:** ADR 0034 (apps), ADR 0036 (passwords), ADR 0037 (the model connection),
  ADR 0048 (outside tools), ADR 0052 (what an agent may do)

The owner asked (2026-11-14): "if they already have local AI configured (like claude or codex,
or cursor), what can we do to streamline their onboarding? … their experience should be one
that wows them." Someone with one of those tools has usually done the tedious parts once:
connected GitHub, Linear and the rest to that tool, and often run a local model or exported a
provider's key. Ghost's first run made them do all of it again.

## Decision

**Ghost reads what the other tools set up, and offers each piece once, by the operator's own
click** (`ghost_core::other_tools`, `GET /other-tools`). Three things are offered, each shown
before it is done:

1. **The apps the other tools use.** Each entry is shown as what it is: an address without its
   query or user-info, or the exact command Ghost would run, word by word (someone else's text,
   in mono, never run or connected before the click). Connect writes one `apps.json` entry
   through the one path the desktop's connect already takes (`apps::connect_specs`, the single
   writer `edit_apps`); every secret in the entry, a header named for a key, a setting named for
   one, an argument or a query parameter, the address's user-info, goes straight into Ghost's
   vault under its own name, and the entry carries `${NAME}`. An entry that points at Ghost
   itself, is named `ghost`, or is the same as an app already connected (the same address
   without its query, or the same command and arguments) is not offered, and says why. An
   address with no key of its own is offered as a sign-in through Ghost's own flow; the other
   tool's stored sign-in is never copied. A second import of the same apps changes nothing.
2. **Ghost as a tool of theirs.** "Let Cursor use Ghost" makes a client exactly as `ghostd mcp
   add` does (`ghost_app::clients`) and adds Ghost to that tool: Claude Code and Codex by their
   own command when it is on the PATH (`claude mcp add --scope user ghost -- …`, `codex mcp add
   ghost -- …`), run as argv. **Ghost never writes `~/.claude.json`**: Claude Code rewrites that
   file itself and keeps its own account state there, so without `claude` on the PATH Ghost makes
   the client and shows the exact `claude mcp add …` command for the operator to run (in the app
   and from `ghostd setup link`), and the same for `claude mcp remove` on unlink; the one file
   writer refuses that file whatever plan reaches it. Cursor's `~/.cursor/mcp.json` and Codex's
   `config.toml` (without `codex`) are merged: every other entry and key kept in order, the
   file's mode kept, a timestamped backup beside it first, and the new file put in place by one
   rename. Where `ghostd` stays put the tool runs `ghostd mcp serve --client <name>` and its file
   holds no token; from an AppImage it connects by address with the token in a header, as the
   operator would have pasted it (and the command shown to run carries it, once). A file with
   comments (which Ghost would lose) is not written: the entry is shown to add by hand. Removing
   the client in Settings › Outside tools offers to take the line back out
   (`DELETE /other-tools/{tool}/link`).
3. **A model connection that is already there.** Ollama or LM Studio answering on their default
   loopback ports, with their model lists, and `ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY` or
   `OPENAI_API_KEY` set in the environment Ghost was started in (which Ghost reads already, ADR
   0037). The model step offers each as one click that fills its draft; Save and continue is the
   operator's as before.

**The operator's only.** The routes are the control API's; `ghostd setup detect [--json]`,
`ghostd setup import <tool> <app>… | --all`, `ghostd setup link <tool>` and `ghostd setup
unlink <tool>` reach them through the running daemon for a terminal or a coding agent following
the setup guide, each printing what it will do and asking y/N unless `--yes`. There is no tool
of Ghost's own app for any of it, nothing for outside clients and nothing on the phone
(`no_tool_reads_or_writes_the_other_tools_on_this_computer`,
`nothing_an_outside_client_must_not_reach_is_offered`,
`the_phone_cannot_reach_the_other_tools_on_this_computer`). This differs from
`ghost_app_connect` (ADR 0052) in who acts and what moves: there an agent connects one app from
the catalog, asking first, and a key stays the operator's to type; here the operator, at the
desk, moves a key they already wrote into another tool's file into Ghost's vault with their own
click. No model is involved anywhere in this (CLAUDE.md, "Deterministic first").

## What is read, and what is never read

Read, and only these (`other_tools::files_for`, `read::read_bounded`), each from the tool's own
documentation:

| Tool | File | What |
|---|---|---|
| Claude Code | `~/.claude.json` | `mcpServers` (user scope) and `projects.<path>.mcpServers` (local scope); 16 MB at most |
| Claude Code | `<project>/.mcp.json` | project scope, for at most 32 projects `~/.claude.json` names inside the home folder; 1 MB |
| Cursor | `~/.cursor/mcp.json` | `mcpServers`; comments and trailing commas read; 1 MB |
| Codex | `$CODEX_HOME/config.toml`, else `~/.codex/config.toml` | `[mcp_servers.<name>]`; 1 MB |

and a GET of `http://127.0.0.1:11434/api/tags` and `http://127.0.0.1:1234/v1/models`, 800 ms
each, no redirects, no proxy. Whether a tool is set up is a stat of its folder or its command
on the PATH, never a read.

**`~/.claude.json` is read for its app list only.** It also holds Claude Code's account (an
email, an organisation, an id, perhaps a key) and each project's history: Ghost deserializes only
the two `mcpServers` maps into types of its own with no other field
(`other_tools::claude::read_claude_json`), drops the text before the function returns, and what
leaves it holds the app entries and the project folders they belong to, nothing else
(`nothing_but_the_apps_leaves_claude_json`, and through the API, the vault and `apps.json`,
`nothing_of_claude_codes_account_reaches_the_api_the_vault_or_apps_json`). A project's folder is
shown only as a short label (`~/code/site only`, 60 characters at most, redacted) and never
written into `apps.json`.

**Never opened:** another tool's sign-in. Not Claude Code's or Codex's subscription tokens,
not Cursor's session, not a keychain: `~/.claude/` (all of it, `.credentials.json` included),
`~/.codex/auth.json`, `~/.config/Cursor/`, and any file named for credentials, a token, a key
or `auth.json`, whatever path or link leads to it. Those are licensed to that tool; the model
step says so in one sentence where it shows "Claude Code is installed", and points at a key,
OpenRouter's sign-in or a local model. The one function that opens a file refuses them, and
`a_sign_in_file_is_never_opened` plants each one and checks every path opened.

**A link out of the home folder is not followed**; one inside it (a dotfiles folder) is. A file
over its bound, not JSON or TOML, or not text, is one line saying so; it is never a crash, and
it is never written. Everything read is someone else's text: each value cut to 300 characters
and redacted of every password Ghost holds, shown as data, never placed in a prompt. The
response is names and kinds: never a secret's value, an address without its query or
user-info. Another tool's variables are read as Ghost reads its own (`${VAR:-default}` is
`${VAR}`, with a note; `${env:NAME}` is `${NAME}`; `${userHome}` the home folder); one only
that tool knows (`${workspaceFolder}`, `${input:…}`, a plugin's root) is a reason not to offer
the entry. An entry that connects over SSE or WebSocket, gets its headers from a script, or is
turned off in its tool is named and not offered.

## What is written, and where

- `~/.config/ghost/apps.json`, through `edit_apps` as any connect, and the vault, for each app
  the operator connects.
- The client in Ghost's store, and for the command form its token file under
  `~/.local/share/ghost/clients/` (0600), as `ghostd mcp add --command` writes.
- The other tool's own settings, only when the operator lets it use Ghost or takes Ghost back
  out: by its command (`claude mcp add|remove --scope user`, `codex mcp add|remove`), run as
  argv with none of Ghost's passwords in its environment and a 30 s bound, or in Cursor's
  `~/.cursor/mcp.json` or Codex's `config.toml`, with the old file kept beside it as
  `<name>.ghost-backup-<time>`. Never `~/.claude.json`: without `claude`, the operator runs the
  command Ghost shows. A failure takes the client back out, and the file is as it was.
- Nothing of what is read reaches a log line: the import logs a count, and
  `no_log_line_carries_a_token_from_another_tools_file` captures every level during detection
  and import from the hostile fixtures and finds none of their tokens.

## Consequences

- A newcomer with Claude Code, Cursor or Codex sees one calm line at the top of the app step
  ("Claude Code and Cursor are set up on this computer.", Wisp looking up) and the apps found,
  in the step's own list, above the catalog; Apps shows the same under "In your other tools".
  Someone with none of them sees exactly the first run they saw before.
- A key the operator wrote into another tool's file is now also in Ghost's vault. Changing it
  in one place does not change the other.
- An import shares the clash and locked-keyring rules of any connect: a locked keyring refuses
  it before `apps.json` changes.
- VS Code and Windsurf are not read: VS Code's documentation gives no Linux path for its user
  `mcp.json` and calls the format deprecated, and Windsurf's documentation moved while this was
  built. Each can be added as one entry of `files_for` and its parser when its format is
  documented.
- Cost on a turn (ADR 0018): nothing. Detection runs when the first run's steps, Apps or
  Settings › Outside tools open, and on Look again: a few file reads and two loopback GETs of at
  most 800 ms each, run off any session.
