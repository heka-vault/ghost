# 0037: One model connection, over the Anthropic or an OpenAI-compatible API

- **Status:** accepted (decided 2026-10-28, #34)
- **Date:** 2026-10-28
- **Amends:** ADR 0018's endpoint switch (`GHOST_LLM_ENDPOINT=proxy|openrouter`), which keeps
  working as the older form of the connection
- **Builds on:** ADR 0036 (the connection's key is a password in Ghost's vault)
- **Amended by:** ADR 0056 (Test connection is the provider check, kept per route; a field
  a route refuses is left out of its requests; a failed turn is said); 2026-11-13 below (what
  the desktop is told is the connection Ghost uses; Passwords says it, and `ghostd` reads
  `settings.env` itself); 2026-11-14 below (the named `cliproxy` provider removed, its saved
  value read as `custom`)

Ghost reached its model through whatever `ANTHROPIC_BASE_URL` pointed at, or OpenRouter's
Anthropic API with `GHOST_LLM_ENDPOINT=openrouter`. Only Anthropic's protocol was spoken, the
settings were environment variables the operator had to know, and a watch's model step built a
client of its own. The operator wants to pick a model the way he connects an app: a provider,
a sign-in or a key, a model from its list, and a test that says how fast it answers.

## Decisions

**One connection for everything that uses a model.** Spoken sessions with their tool calls,
compaction (Pipecat's summarizer runs on the session's own service), a watch's model step and
the latency harness all resolve the same connection (`ghost_core.connection`). Jev, the
confirmation classifier, keeps its own key and path (ADR 0017); it is not this connection.

**A provider, picked like an app.** `GHOST_LLM_PROVIDER` is one of `anthropic` (a password),
`openrouter` (sign in), `cliproxy` (the operator's Claude subscription through CLIProxy on his
network), `openai`, `ollama`, `lmstudio`, or `custom` (an address that speaks either protocol,
`GHOST_LLM_PROTOCOL`). `GHOST_LLM_MODEL` is the model as the provider names it, picked from
its own list (`/v1/models`, or `/models` under an OpenAI-compatible base), with a default per
provider. Folded away: `GHOST_LLM_BASE_URL`, `GHOST_LLM_HEADERS` (`Name: value; …`, sent as
written, never a key) and `GHOST_LLM_TIMEOUT_SECS`. All of it is non-secret and lives in
settings.env.

**The key is a password in Ghost's vault, named, never shown** (ADR 0036). Each provider has
its own (`ANTHROPIC_API_KEY`, `OPENROUTER_API_KEY`, `ANTHROPIC_AUTH_TOKEN` for CLIProxy,
`OPENAI_API_KEY`, `LLM_API_KEY`), all in `GHOST_USES` as Ghost's own; `GHOST_LLM_PASSWORD`
names another, which the Passwords page then shows as the model connection's. The Model page
saves the key on its own row, through `PUT /passwords/{name}`, like the Passwords page. Like
Ghost's other own passwords it reaches the connection through the daemon's environment, in
memory, which the vault fills at start and updates on every save; it is read by name when a
session is built, so a key saved from the desktop is used by the next session without a
restart (`OPENROUTER_API_KEY` still asks for one, for Jev). A list or a test notes it as
used; redaction covers it like every other value in the vault. Keys for providers not in use
are listed on the Passwords page once they are set. It never comes back out: views say set
or not set.

**OpenRouter by signing in.** OAuth with PKCE (S256): the desktop opens
`openrouter.ai/auth` with a `localhost` callback on Ghost's sign-in loopback (the one apps use,
`ghost_core.signin`); Ghost trades the code at `/api/v1/auth/keys` for a key and keeps it in
the vault as `OPENROUTER_API_KEY`. A callback carrying another request's `state` is refused,
and no sign-in starts while the keyring is locked, since the key would have nowhere to go.
There is nothing to copy.

**One client interface, two implementations** (`ghost_voice.llm.model_client`): the session's
Pipecat service, a one-shot completion, and a first-token probe, for the Messages API and for
Chat Completions. Pipecat's adapters translate the context and tool calls both ways. Both keep
every fix the Anthropic path earned: the 2000-token output cap, every tool call of a reply
(Pipecat's OpenAI service reads only the first call of each chunk, and Ollama sends a reply's
calls in one chunk), no half batch and one redo in smaller steps when a reply stops at the cap
(`finish_reason: length`, or a stream that ends inside a call's arguments), and the `cut_off`
flag. OpenRouter stays on its Anthropic API, so the system-prompt cache marker keeps working.

**Test connection is the latency check's own measure.** One short streamed request, timed
from the call to the first content event, as `make perf-llm` times it; the saved connection
goes over the client sessions share, a draft over one that isn't kept. A key typed but not
yet saved may ride along in that one request.

**The older settings keep working.** With no `GHOST_LLM_PROVIDER`, `GHOST_LLM_ENDPOINT=
openrouter` is OpenRouter with `OPENROUTER_API_KEY`, and `proxy` (the default) is
`ANTHROPIC_BASE_URL` with `ANTHROPIC_AUTH_TOKEN` (shown as CLIProxy when it is
`CLIPROXY_BASE_URL`), or Anthropic itself when no address is set. The desktop shows such a
setup as the provider it resolves to and writes it out as that provider on its first edit.

## Consequences

- Nothing is added to a turn: the target is resolved once per session (a dictionary lookup),
  the SDK client is shared per connection, and the watcher that keeps every tool call reads
  chunks already in hand. `make perf-llm` before and after on OpenRouter: hello TTFB p50 1.27 s
  and 1.11 s, within run-to-run noise.
- An OpenAI reasoning model (`gpt-5*`, GPT-6, the o-series) counts its reasoning against
  the cap. Over Chat Completions it is sent the lowest `reasoning_effort` it takes with
  tools (`none` from GPT-5.1 on; `low` for the o-series and the first GPT-5s, whose
  `minimal` turns off parallel tool calls) and 6000 tokens of room on top of the 2000. It
  is a property of the model id wherever it is served (`ghost_core.connection.reasoning`),
  and has not been tried live yet.
- A model reached over Chat Completions gets no Anthropic cache markers; a Claude model is
  better reached through a provider on the Messages API.
- `ghostd perf run` and `perf llm` take `--endpoint current` (the default) or a provider id.
- heka-vault's one-time import (ADR 0036) also fetches `ANTHROPIC_API_KEY` and
  `OPENAI_API_KEY`.

## Amendment, 2026-11-13: the connection reported is the one in use

A newcomer started Ghost from a shell with a provider's address and key exported: Ghost
answered, while the first run, Today and Settings › Model said it needed a model. The
sessions read the environment Ghost was started in with its own over it (the vault's values,
the settings saved since); `GET /model` read the vault and `settings.env` only. Now both
resolve one environment (`ghost_core::environment::resolve`: `Daemon::env` and
`Deps::ghost_env`), so `ready`, the provider and whether the key is set are what a session
uses. A setting cleared on the Model page is kept as "" over the start's value, so it stops
counting without touching the process environment. `ModelConnectionView.from_environment`
names the settings and the password the connection reads only from the environment Ghost was
started in (not `settings.env`, not the vault): names, never a value. Settings › Model says so
("From the environment Ghost was started in") with the way to keep them: save the page for the
settings, add the password to the vault. Nothing is written to an env file by Ghost itself, and
a value still never comes out (ADR 0036).

## Amendment, 2026-11-13 (later): the same on Passwords, and `ghostd` reads `settings.env`

**Settings › Passwords says it too.** `PasswordInfo.from_environment` is true for a name
that is not in the vault and whose value Ghost is using from the environment it was started
in (the same `from_start` test as `/model`). The row says "From the environment" in place of
"Not set", with "until Ghost is started another way. Add it to keep it."; `ghostd passwords
list` prints the same with `ghostd passwords set NAME`. Names only, never a value.

**Every `ghostd` command loads `settings.env` itself**, first, before any thread
(`ghostd::config::load_settings_file`, over `ghost_core::environment::settings_file_under`).
A plain `ghostd run` from a shell read only what the shell exported, while the launchers and
`ghostd mcp` read the file. The precedence: a setting in the file applies unless the process
environment holds the name, and then the process environment's value wins, an empty one
included. The file is what the operator saved in Ghost; a variable present at the start was
put there for that start (a shell's export, a unit's `Environment=`), the one way to override
a saved setting without editing it, and it is exactly what `from_environment` reports. A
password's name in the file is left out with a log line (ADR 0036). A launcher that loads the
file itself (`make` and `make dev`, the user unit and the deploy drop-in, the desktop app)
sets `GHOST_SETTINGS_LOADED=1` and the daemon reads nothing more, so the file is read once
and each launcher's own rule stands (the file over the login session's exports, a `./.env` in
its place for the make targets).

## Amendment, 2026-11-14: the named `cliproxy` provider is removed

The owner's decision: Ghost does not offer or describe a named proxy provider. `cliproxy` is
no longer one of the providers (`ghost_core::connection::PROVIDERS`), the desktop does not
offer it, and no document a user reads names it. The ways to connect are a key (Anthropic,
OpenRouter, OpenAI), OpenRouter's sign-in, a model on this computer (Ollama, LM Studio), or
your own address (`custom`), which reaches any Anthropic- or OpenAI-compatible address.

A `settings.env` that still says `GHOST_LLM_PROVIDER=cliproxy` keeps working without a word:
`ModelConnection::from_env` reads it (`LEGACY_PROVIDERS`) as `custom` on Anthropic's protocol
with the route it had (`GHOST_LLM_BASE_URL`, else `CLIPROXY_BASE_URL`, else
`http://localhost:8317`; `GHOST_LLM_PASSWORD`, else `ANTHROPIC_AUTH_TOKEN`; the same model and
headers). `GET /model` reports it as `custom` with that address as its own, so Settings shows
"Your own address" and its first save writes `custom`, never `cliproxy`. With no provider set,
`ANTHROPIC_BASE_URL` is `custom` whatever `CLIPROXY_BASE_URL` says. A kept model check from
before is named as `custom` in the slow note's advice. `make perf-providers` and `perf llm`
time your own address as `custom` (`GHOST_LLM_BASE_URL`, else `ANTHROPIC_BASE_URL`).
