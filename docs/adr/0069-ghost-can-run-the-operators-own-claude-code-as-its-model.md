# 0069: Ghost can run the operator's own Claude Code as its model

- **Status:** accepted
- **Date:** 2026-11-14
- **Builds on:** ADR 0037 (one model connection), ADR 0056 (each connection shaped and
  checked), ADR 0068 (the AI tools already on the computer)
- **Amends:** ADR 0068 (Claude Code's sign-in in the model step), ADR 0049 and ADR 0054
  (speculative answers, off for this provider), ADR 0050 and ADR 0062 (the fast model and the
  race, off for this provider)

The owner (2026-11-14): "Give people the [Claude Code] option up front… the goal is to meet the
user where they are, not block, reduce the steps it takes to get to the first 'hey ghost'." The
first run's model step asked for an API key. Most people likely to install Ghost already have
Claude Code installed and signed in with a Claude plan.

## Decision

**A provider, `claude-code` ("Claude Code"), runs the program the operator installed and signed
in, as it is, and nothing else of it.** No address, no password, no fast model. When the
program is signed in, the first run's model step leads with it: already selected, applied only
by its own button (Use Claude Code), every other way one click away under Use something else.

### What Ghost runs

One process per session, and one per run with nobody there (ADR 0047), started when the
session opens (during the greeting) and stopped when it ends:

```
claude --print --input-format stream-json --output-format stream-json --verbose
       --include-partial-messages --system-prompt-file <0600 file>
       --tools "" --setting-sources "" --strict-mcp-config --mcp-config <0600 file>
       --allowedTools=mcp__ghost__* --disable-slash-commands --no-session-persistence
       --permission-mode dontAsk --settings {"alwaysThinkingEnabled":false}
       --effort low --model <alias>
```

- `--system-prompt-file`: Ghost's whole prompt (the agent's persona, its memory, the operator's
  rules) is in a 0600 file, never on the command line, which every user of the computer can
  read (`/proc/<pid>/cmdline`). The relay's config and bearer are a 0600 file too. Both are in a
  0700 folder per process under `$XDG_RUNTIME_DIR/ghost/claude-code/`, removed when the process
  ends; the folders a stopped Ghost left (a crash, a kill) are swept at the next start. The
  program's `--help` lists this switch only inside `--bare`'s text, so detection asks the
  program's parser (`<switch> /dev/null --ghost-probe-unknown` names the probe as the unknown
  option only when the switch is known); a version without it is refused, never given the
  prompt on argv. Nothing else of the operator's is on argv: the model's alias and the fixed
  `--settings` JSON.
- `--tools ""`: the program's built-in tools (files, shell, web) are off.
- `--setting-sources ""`: the operator's user, project and local settings, and with them
  their hooks and plugins, are not loaded. `--disable-slash-commands`: no skills or commands.
- `--strict-mcp-config --mcp-config <file>`: the only app the program has is Ghost's relay
  (below). `ENABLE_CLAUDEAI_MCP_SERVERS=false` keeps the apps of the operator's claude.ai
  account out too.
- `--allowedTools=mcp__ghost__*` (the `=` form: the flag is variadic) and `--permission-mode
  dontAsk`: the relay's tools are allowed, anything else is refused, never asked.
- `--no-session-persistence`: nothing of the conversation is written to the program's folder.
- `--effort low`: the program's own list says thinking can't be turned off on the current models
  (`supportsThinkingOff: false`); low effort measured the quickest first word.
- The working folder is an empty temp folder (0700). The environment is built from scratch:
  `HOME`, `PATH`, `USER`, `LANG`, `ENABLE_CLAUDEAI_MCP_SERVERS=false` and `MCP_TOOL_TIMEOUT`
  (ten minutes, so a change waiting for the operator's yes is not cut off). Nothing named
  `ANTHROPIC_*` or `CLAUDE_*` is inherited: on the workstation an inherited proxy token silently
  became the program's credential.
- `--bare` is not used: it never reads the operator's sign-in.

Every switch Ghost passes is one the detection requires of the program, from its own `--help`
or its parser (`program::REQUIRED_SWITCHES`); a version without one is refused with a sentence and never run
with its own tools on. Another major or minor than the 2.1 Ghost was measured with runs when it
has every switch, with a one-line note; refusing a newer version that works would block the
operator for nothing, and running one without a switch is never allowed.

A compaction and a watch's model step run the program once with the same switches and no app
(`--output-format json`).

### Tools reach the gate by a relay; the program runs nothing

The relay (`ghost_llm::claude_code::relay`) is a Model Context Protocol endpoint per process, on
a loopback port of its own: not the control API, the phone's listener, the webhook listener or
the listener for outside tools, and none of their routes. Every request carries a bearer of 32
random bytes, kept in memory and in the program's 0600 app file only, compared in constant time,
dead with the process; a request with an `Origin`, or a `Host` that is not that port, is refused.
It offers exactly the session's tools as the stage last sent them, under Ghost's own names
(the program calls them `mcp__ghost__<tool>`; the call arrives under Ghost's name, so nothing is
read back out of a prefixed name), and tells the program when they change
(`notifications/tools/list_changed`, ADR 0046). A call to a tool not on the list is refused.

A call is **parked**: it comes out of the backend's stream as the same `Event::ToolCall` an HTTP
model's reply carries, so the stage, the dispatcher, the gate, acknowledgments, the audit and
cancellation above it do not fork. The result the stage would send in its next model call is the
parked request's answer. Nothing the program sends is the operator's yes: a change at `ask` waits
on the gate as any change does (and in a run is set aside), and the relay never confirms one.
A turn the stage drops (the operator talked over it) refuses every parked call ("Not run"),
interrupts the program with the Agent SDK's control request (`{"type":"control_request",
"request":{"subtype":"interrupt"}}`), and reads it to its turn's end before anything else is
written. A change is never dropped mid-gate: the dispatcher runs calls inside
`ghost_core::apps::with_cancel` as it always has.

### Context: the program keeps history; Ghost's transcript stays the record

The program keeps the conversation; Ghost's context and transcript stay what they are (ADR
0014). The backend gets Ghost's whole context on every call, like every model, and remembers
what it has given the program. What is new is fed: a tool's result to its parked call, the
operator's words as a turn. When Ghost's context no longer begins with what was given, the
process is started again and the history before the turn is given as that turn's first lines
("Operator: …", "You: …", "(you called …)"); what a tool returned is left out of it, since it was
an app's text and stays inside the bounds it was given in. So:

- **Speculative answers** (ADR 0049, 0054) are **off** for this provider: a speculative turn fed
  to the program can't be taken back. Cost: the speculative head start (a few hundred ms on a
  quick close) is lost.
- **Barge-in:** the reply is interrupted; the program keeps what it had written, Ghost what was
  heard. The next turn carries Ghost's note that it was said over the reply.
- **Compaction:** Ghost's context changes, so the process starts again with Ghost's summary
  (a user message, as on every connection) as the first lines.
- **A session that resumes** with stored history starts a process whose first turn carries it.
- **A handover to another agent** changes the system prompt, so the process starts again with
  the new agent's prompt and history: one process per agent at a time, at a cold start's cost
  (about 0.2 s, below) on each handover.
- **A change of the prompt's app paragraphs** (an app connected mid-session) restarts the process
  at the next turn; the tool list alone changes through `list_changed`, with no restart.
- **A run with nobody there** gets a fresh process, the same relay and gate; its 30 model calls
  and ten minutes are counted and enforced by Ghost as before.
- **The fast model and the race** (ADR 0050, 0062) are **off**: the program answers every call.
  Cost: the fast model's quicker first word on small talk; the program measured about the same
  as the direct connection's main model anyway (below).

### Detection, health and failures

`other_tools::program` asks the program, never its files: `claude --version`, `claude --help`
(the switches), `claude auth status --json` (into four fields: signed in, the method, an API
key's source, the plan's name; never the account's address). Not installed: the option is not
shown. Installed and signed out: "Claude Code is installed but not signed in. Run `claude auth
login` in a terminal, then Look again." Never a sign-in button or flow inside Ghost. Signed in
with a plan: replies count against its limits. Signed in with an API key: still fine, it bills
that key, and Ghost says so. `ghostd setup detect` reports it. The model check (ADR 0056: `POST
/model/check`, Settings › Model's Test, `ghostd llm test`) times it through one process of its
own: ready, first word over a few turns, streaming, a tool round trip through the relay, and the
cache as the program reports it.

Failures are said in plain words (`spoken_failure`), never silence, and reported by kind only
(ADR 0067: `turn_failed` with `provider` and `reason`): the plan's limit reached, the sign-in
expired mid-session (Ghost stops at the program's first refused retry instead of waiting out its
ten), the program missing or changed to a version without a switch, the process dying (the next
turn starts a fresh one with the history). The program's own words are never repeated, logged
or reported.

### What Ghost never does

Read, copy or store the program's sign-in or any file of it; write its settings; modify,
patch or wrap the binary; run its built-in tools; pass it Ghost's passwords or the environment's
`ANTHROPIC_*`/`CLAUDE_*` variables; offer a sign-in, or say "Sign in with Claude"; say or imply
that Anthropic made, endorses or partners with Ghost; say it is free or unlimited. No tool of
Ghost's own app, no outside client and nothing on the phone can choose this provider or reach the
relay (`no_tool_chooses_the_model_or_reaches_claude_codes_relay`,
`the_phone_cannot_choose_the_model_or_reach_claude_code`). "Ghost never switches provider or model
itself" still holds.

## What it costs (2026-11-14, ADR 0018)

On the workstation, Claude Code 2.1.325, Haiku (`--effort low`), small synthetic prompts, an
empty temp folder (`cargo run -p ghost-llm --example claude_code_perf`). The program's own
sign-in had expired and Ghost must not sign it in, so **for measurement only** the harness gave
the program an address and token of its own (`Settings::harness_env`, never set by the daemon):
these time what the program adds, and the subscription path itself is unverified here. The
direct connection went through the same address.

| | Claude Code | Direct |
|---|---|---|
| Process start to ready (relay connected) | 0.21 s median (0.21–0.32, n=5) | – |
| First word, first turn after ready | 0.71 s (0.69–0.90, n=5) | – |
| First word, warm | 0.65 s (0.51–0.81, n=10) | 0.66 s (0.61–0.72, n=4) |
| A tool call at the stage | 0.58 s (0.55–0.61, n=2) | 0.73 s (n=1; one 1.95 s outlier) |
| A one-tool turn, call to the answer's first word | 1.18 s (1.13–1.23, n=2) | – |
| The answer after a result | 0.61 s (0.50–0.76, n=5) | – |
| Barge-in to the program idle (interrupt and drain) | 3 ms (3–22 ms, n=5) | – (the request is dropped) |

The relay over HTTP against an app over stdio (a Python stand-in, the same prompts): the first
tool call reached the app at 0.57 s either way; with `initialize` sent at the start both are
connected before the first turn (HTTP 0.27 s, stdio 0.44 s), which removes the 3.5 s a cold
process with a stdio app took to its first call. A 60-character tool name (72 with the prefix)
works. Haiku measured the quickest to a first word and a tool call, so it is the default alias;
Sonnet with `--effort low` was about 0.6 s warm and 1.0–1.1 s to a tool call.

What is lost against a direct connection: the speculative head start and the fast model (above);
Ghost's own prompt-cache warm-up (the program places its own markers; the warm turns read the
cache); the per-route shaping of ADR 0056 (thinking, effort, cache markers are the program's);
the thinking setting (only effort); the program's own prompt on top of Ghost's (in these
recordings about 370 input tokens with no app and about 850 with one small tool, on every
request, mostly read from the cache), which counts against the plan; and exact token counts per
turn, which come from the program's report.

## The vendor's position, and why Ghost offers it anyway

Read on 2026-11-14:

- [Legal and compliance, "Authentication and credential use"](https://code.claude.com/docs/en/legal-and-compliance):
  "OAuth authentication is intended exclusively for purchasers of Claude Free, Pro, Max, Ultra,
  Team, and Enterprise subscription plans and is designed to support ordinary use of Claude Code
  and other native Anthropic applications." and "Anthropic does not permit third-party
  developers to offer Claude.ai login into their own applications, or to route requests through
  Free, Pro, Max, or Ultra plan credentials on behalf of their users. Moreover, developers may
  not collect, store, or intermediate Claude.ai credentials or session tokens — sign-in to a
  Claude account must complete through Anthropic's own flow." and "Nor does it prevent an end
  user from signing in to the unmodified Claude Code binary with their own Claude subscription,
  including where a platform hosts Claude Code as described under *Can customers offer Claude
  Code in their products?* above." The same page, on offering Claude Code in a product: "The
  Claude Code binary must not be modified." and "You can accurately say, in plain text, that your
  product has Claude Code preinstalled or that it runs Claude Code. But you can't use the Claude
  Code or Anthropic names or logos as part of your own product, feature, or company name, … or in
  a way that suggests Anthropic built, endorses, or is partnered with your product."
- [Agent SDK overview](https://code.claude.com/docs/en/agent-sdk/overview): "Unless previously
  approved, Anthropic does not allow third party developers to offer claude.ai login or rate
  limits for their products, including agents built on the Claude Agent SDK."

Ghost runs the operator's own unmodified program, signed in by the operator through the
program's own flow, on the operator's computer, for the operator alone; it offers no login,
collects or stores no credential, and does not pay for, resell or route anyone else's usage. It
says plainly that it runs Claude Code, in plain text, and that replies count against the
operator's plan. Whether that is "ordinary use" of a plan is not settled by the published text,
and the SDK page's sentence on rate limits can be read against it. **The owner chose to offer it
knowing that position is unsettled.**

**Ghost withdraws it** (removes the provider, or stops offering it up front) if Anthropic says
this use is not permitted, by its published terms or to the owner; if the program changes so
that Ghost can't run it with its own tools, settings and apps off; or if the program starts
requiring a credential Ghost would have to handle.
