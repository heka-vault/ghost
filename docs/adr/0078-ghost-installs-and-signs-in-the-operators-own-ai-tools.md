# 0078: Ghost installs and signs in the operator's own AI tools, on their click

- **Status:** proposed
- **Date:** 2026-11-25
- **Builds on:** ADR 0013 (the control API), ADR 0036 (passwords), ADR 0037 (one model
  connection), ADR 0068 (the AI tools already on the computer), ADR 0069 (the operator's own
  Claude Code as the model), ADR 0077 (dictation and its tidy)
- **Amends:** ADR 0069 ("Never a sign-in button or flow inside Ghost": the program's own
  sign-in may now be started from Ghost, see below), ADR 0068 and ADR 0037 (the first run's
  model step and Settings › Model offer the subscriptions only, for now)

The operator (2026-11-25, issue #189), after the first run and Settings › Model: "That screen is
atrocious. The way that we do onboarding is very confusing." And what it should be: "Choose the
subscription first: Claude Code, OpenAI Codex, or Google Antigravity. If the program isn't
installed, a button installs it. If it's not signed in, a button signs in: it opens the browser,
and sometimes I have to paste a code back. We are not an alternative harness; we use their tools
according to their policies."

## Decision

**The model step and Settings › Model are one picker of the operator's own subscriptions**
(`ghost_core::subscriptions`, `/subscriptions` on the control API, `SubscriptionPicker` in the
desktop): Claude Code, OpenAI Codex and Google Antigravity, each the vendor's own program, one
card each with its state and one action: **Install**, **Sign in**, **Use it**, and which one
Ghost uses now, said plainly. An API key, a local model and an address (ADR 0037) are not offered
there for now; the code and the settings for them stay, and a Ghost already set up that way keeps
its connection, folded under "Your connection now" on Settings › Model.

### One fixed table, the vendors' own commands

| | Program | Install (the vendor's own, run as given) | Signed in? (asked of the program) | Sign-in |
|---|---|---|---|---|
| Claude Code | `claude` | `curl -fsSL https://claude.ai/install.sh \| bash` ([setup](https://code.claude.com/docs/en/setup)) | `claude auth status --json` (`program::look`, ADR 0069) | `claude auth login`, in a terminal of Ghost's |
| OpenAI Codex | `codex` | `curl -fsSL https://chatgpt.com/codex/install.sh \| sh` ([Codex CLI](https://learn.chatgpt.com/docs/codex/cli)) | `codex login status`: exit 0, not "Not logged in" | `codex login`, in a terminal of Ghost's |
| Google Antigravity | `agy` | `curl -fsSL https://antigravity.google/cli/install.sh \| bash` ([codelab](https://codelabs.developers.google.com/antigravity-cli-hands-on)) | `agy models`: exit 0, not asking to sign in | `agy`, run by the operator in their own terminal |

Each installer puts its program in `~/.local/bin`; Ghost finds a program on its PATH, else
there (`subscriptions::which`), and so does the Claude Code provider.

**Asked, never read.** Detection runs the program (`--version`, its status command), as argv, in
an environment built from scratch (`subscriptions::base_env`): `HOME`, `PATH` with
`~/.local/bin`, `USER`, `LANG`, for Claude Code the switch that keeps claude.ai's apps out, for
Codex `CODEX_HOME` when set. Nothing named `ANTHROPIC_*`, `CLAUDE_*`, `OPENAI_*`, `GEMINI_*`,
`GOOGLE_*`, no key, none of Ghost's passwords: on the workstation the shell's
`ANTHROPIC_AUTH_TOKEN` made `claude auth status` say signed in when it was not. No file of a
program's is opened; `a_sign_in_file_is_never_opened` and ADR 0068's list hold as before.

### Install, on the click

The card shows the exact command and where the vendor documents it, and asks first. Ghost then
runs it with `sh -c` as the operator, never as root (refused when Ghost runs as root), in the
same clean environment, in its own process group, ten minutes at most. Its output (both streams)
is the job's lines: escape and control sequences out, 200 lines of 300 characters at most, each
redacted. Cancel stops the group. When it ends, Ghost looks for the program again.

### Sign-in, the program's own flow

Ghost runs the program's own sign-in in a pseudo-terminal of its own (`rustix`'s pty, already
in the tree): a new session with the terminal as its controlling one, echo off, 1000 columns so
an address is never wrapped. The environment is the clean one plus what a browser needs to open
(`DISPLAY`, `WAYLAND_DISPLAY`, `XDG_RUNTIME_DIR`, `DBUS_SESSION_BUS_ADDRESS`, `XAUTHORITY`,
`XDG_CURRENT_DESKTOP`, `XDG_SESSION_TYPE`, `BROWSER`) and `TERM`. The program opens the browser
itself, as it would in the operator's terminal. What it prints is read as the install's is, and:

- **The address** is taken only when it is https, on that program's own vendor domains
  (`claude.ai`, `claude.com`, `anthropic.com`; `openai.com`, `chatgpt.com`; `google.com`,
  `antigravity.google`), with no user, password or other port, from a line or an OSC 8
  hyperlink. It is the job's `url`, whole, for an **Open the sign-in page** button in case the
  browser didn't open. In the lines it is cut to its host and path: its query is the sign-in's.
- **A code the page shows, pasted back.** When the program leaves a prompt open that asks for a
  code (`Paste code here if prompted >`), the job is `needs_code` and the card shows a field.
  The code goes to `POST /subscriptions/{id}/sign-in/code`, is checked as one printable line of
  512 characters at most, written to the terminal with Enter, and held in memory (zeroized) only
  to hide its echo until the program ends: never in a response, a log line at any level, the
  audit, the store or the lines (`hide_code`). The field is cleared as it is sent.
- **A code to type elsewhere** (a device code, `ABCD-EFGH` on a line about a code) is shown.
- Fifteen minutes at most; Cancel stops the program's whole process group; one job per program
  at a time. When it ends Ghost asks the program whether it is signed in now.

Antigravity's sign-in is its first interactive screen: a menu of ways in, its terms, a folder to
trust. Those are the operator's to answer, so Ghost does not run it: the card shows `agy` to run
in a terminal, and Look again. (Antigravity documents no status or login command as of today;
`agy models` is what its own message points at, and is unverified on this machine.)

### Use it

`PUT /subscriptions/claude-code/use` writes what the first run's Use Claude Code always wrote
(`GHOST_LLM_PROVIDER=claude-code`, the rest of the connection cleared) once the program is
signed in with every switch Ghost needs. Codex and Antigravity have no Ghost provider yet:
409, "Ghost can't use … as its model yet", and the card says **Coming next**.

### We use their tools according to their policies; Ghost is not an alternative harness

Ghost runs the vendor's own program, installed by the vendor's own installer and signed in by
its own sign-in, for Ghost's own small turns (ADR 0069 for how). It never reads, copies or
stores a sign-in; never offers a sign-in of its own in a vendor's name ("Sign in with …"); never
answers a program's own questions for the operator; never says a vendor made, backs or partners
with Ghost, or that a plan is free or unlimited. The picker says so in one line.

### The operator's only

Every route is the control API's, the operator's at their desk: no tool of Ghost's own app
(`no_tool_installs_or_signs_in_another_tool`), no outside client (they are served those tools
only), nothing on the phone (`the_phone_cannot_install_or_sign_in_another_tool`), no run and no
model. The jobs are started only from `ghost-api/src/subscriptions.rs`
(`the_jobs_are_started_only_from_the_control_apis_operator_routes`).

## Dictation's tidy on OpenRouter

Dictation's tidy (ADR 0077 as amended) has 3 seconds. Claude Code started for each call is often
slower than that, so with it as the only model the tidy would mostly time out (issue #189). So:
**with `OPENROUTER_API_KEY` in Ghost's vault, the tidy asks OpenRouter instead of the operator's
model** (`ghostd::dictation::desk::openrouter_target`): OpenRouter's OpenAI-compatible address
(`https://openrouter.ai/api/v1`), a model the operator may name in Settings › Sound › Dictation
(kept with the key's other choices, `tidy_model`), `anthropic/claude-haiku-5.5` unless they do
(listed by OpenRouter on 2026-11-25). The password is set on that page the way the Passwords page
sets one: the value goes into the vault and never comes back out. The same 3 seconds, the same
system prompt, the same `accept` guard; the switch "Tidy with the model" still turns it off, and
`tidy_with` says "OpenRouter". The key is the same one OpenRouter's sign-in on the Model page
keeps, so an operator who signed in there tidies on OpenRouter too.

### Amended 2026-11-25: model selectors

Settings › Model gains, under the subscription Ghost uses (Claude Code for now), **Ghost thinks
with** (Opus, Sonnet, Haiku: Claude Code's own aliases, `GHOST_LLM_MODEL`) and **Quick first
replies with** (None, Haiku, Sonnet: `GHOST_LLM_FAST_MODEL`, ADR 0050), saved through
`PUT /settings` like any setting. The second stays disabled while the model is a program, since
ADR 0069 keeps the fast model off for one. Dictation's tidy becomes an explicit choice, **Tidy
with** (`tidy_source`: off, the subscription with Claude Code's alias, or OpenRouter with a
listed or typed model; `desk::tidy_target`), replacing the rule above that OpenRouter's key
alone turns the tidy there. A choice kept from before reads as it behaved. The guard-rails are
unchanged: 3 seconds, `accept`, nothing kept or logged.

## Consequences

- A newcomer with none of the three installs one, signs in and uses it without a terminal (but
  for Antigravity's own sign-in screen). One with Claude Code signed in sees it ready, one click.
- The first run and Settings › Model no longer offer an API key, OpenRouter's sign-in, a local
  model or an address. They are one switch back (the old `ModelSection` is kept and still serves a
  connection made that way).
- Codex and Antigravity are installed and signed in but not run as Ghost's model until each has
  a provider of its own (its own ADR, like 0069).
- Cost on a turn (ADR 0018): nothing. `GET /subscriptions` asks three programs (Claude Code's
  four questions, two each for the others, 15 s bound each) when the picker opens and on Look
  again; a job is followed by `GET /subscriptions/{id}/job`, which asks no program.
- The tidy on OpenRouter adds one HTTPS call per dictation from the key when the switch is on,
  in place of the main model's call; the words go to OpenRouter and the model's provider.
