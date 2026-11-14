# 0067: Ghost sends error reports from a released build

- **Status:** accepted
- **Date:** 2026-11-14
- **Amends:** the privacy rules in CLAUDE.md ("Idle audio stays on the machine" and the control
  API's) and the rule on bespoke clients ("Outside tools come from apps the operator
  connects"), for error reports only. Copies ADR 0066's "only a released build does this".

The owner is about to cut the next public release and asked to find where and how Ghost did
not work on other people's computers. Until now nothing told us: a panic went to that person's
journal, a page's error to a console nobody opens, and a failed turn into a sentence said once.
The destination is PostHog's error tracking (US cloud). Errors only: no product analytics, no
page views, no autocapture, no session replay, no feature flags, no person profiles.

## Decision

**One sender, in Rust** (`crates/ghost-reports`), used by `ghostd` and by the desktop app's
shell, in both its processes (the main one and `--floating`). A leaf crate, since the shell
does not depend on ghost-core. One place builds the event, one place scrubs, one place sends:
a plain `reqwest` POST of PostHog's `$exception` event to `/i/v0/e/` (its capture API). No
PostHog SDK, in Rust or in the webview: a page hands its error to the shell through the
`report_page_error` command and the shell builds the report.

**What is reported is a closed set.**

- A panic in `ghostd` or in the shell (any thread, any task): the message, the location and
  the backtrace. A panic hook, installed at start, writes it to the spool and does no network
  work; the running sender, or the next start, sends it.
- An uncaught error in a page (the main window and the floating room): `ErrorBoundary`,
  `window.onerror`, `unhandledrejection`. The page hands over its type, message and stack only;
  the shell refuses any other field, a type that is not an error's name, a message over 2,000
  characters or a stack over 16,000, and more than ten a minute.
- A failure of Ghost's own, by kind (`ghost_reports::FailureKind`), with no text from outside:

  | Kind | Where | Facts |
  | --- | --- | --- |
  | `turn_failed` | a turn's model call failed (`ghost_llm::spoken_failure`'s cases, `ghost-voice/src/llm.rs`) | the `LlmError` variant, the status's class (`4xx`), the reason's kind (`ReadTimeout`, `overloaded_error`), the model's name |
  | `speech_models_load` | the speech models are all there and still won't load (`ghostd::daemon`) | none |
  | `speech_models_download` | the download failed, not stopped by the operator (`ghostd::speech_models`) | none |
  | `output_unavailable` | the sound output stopped playing (`output:unavailable`, #117) | none |
  | `input_unhealthy` | the microphone stopped hearing well (ADR 0064) | `dropping` or `deaf` |
  | `update_failed` | "Update and restart" failed (`updater.rs`) | `deb` or `appimage` |
  | `daemon_start_failed` | the shell could not start its daemon | none |
  | `daemon_lost` | the shell's daemon exited while it ran | `signal` or `code`, and its number |

  A fact is a word of at most 48 characters from `[A-Za-z0-9_.:/-]`; anything else is sent
  as `other`. The logger is not hooked: a log line can carry an app's words.

**What every report carries:** Ghost's version, the component (`daemon`, `shell`,
`floating`, `page`), the OS's name and version from `/etc/os-release`, the session type
(`x11`, `wayland`, `tty` or `unknown`), the CPU architecture, how it was installed (`deb`,
`appimage`, `checkout`), and the install id: a random UUID made the first time a report is
built while reports are on, kept in `~/.local/share/ghost/reports/install-id`, deleted when
reports are turned off and made anew when they are turned on. It is the event's
`distinct_id`; it is derived from nothing. Every event sets `$process_person_profile: false`
(no person profile) and `$geoip_disable: true` (the address it came from is not looked up for
a place).

**What a report never carries**, each held by a test: a transcript or anything heard or said,
audio, a typed turn, an app's text, a model's or a provider's message, a password, the user's
name, the home folder's path, the computer's name, an email address, a URL's user, query or
fragment, an IP address, the contents of `~/.config/ghost`, an agent's name or persona. Every
string passes `ghost_reports::scrub::Scrubber`: the daemon's redaction of every vault value
first (ADR 0036), then a URL is cut to scheme, host and path, emails, the home folder (`~`),
the user's and the computer's names, addresses and token-shaped strings (24 or more letters and
digits) are replaced, and the text is cut at 200 characters. A message that is not Ghost's own
is reduced to its type: a panic whose location is not in Ghost's crates, a page error whose
type is not one of the engine's (`TypeError`, `ReferenceError`, `RangeError`, `SyntaxError`,
`EvalError`, `URIError`), and a rejected value that is not an Error. What remains is cut at its
first `": "`, after which code usually formats a value in (`called Result::unwrap() on an Err
value: …`, `JSON Parse error: …`), and whatever it put between backticks or double quotes is
emptied (a slice panic's location is Ghost's own code and its message names the string it was
cutting); a quote left open cuts the message there. Deterministic code only, no model.

**Only a released build sends.** The project's key is compiled in from the build's
environment (`option_env!("GHOST_POSTHOG_KEY")`); `.github/workflows/release.yml` sets it
from the repository secret `POSTHOG_PROJECT_KEY` for the step that builds both binaries, a dry run
too, so the wiring is proven before a tag. Its install test points `GHOST_REPORTS_URL` at
nothing and checks only `ghostd reports status`. With no key there is no sender: nothing is
built, spooled, kept or sent; a checkout's build, a fork's and CI's report nothing. A developer
turns it on in a checkout build by setting `GHOST_POSTHOG_KEY` in the process's environment,
with `GHOST_REPORTS_URL` pointing at a local stub. The key is a PostHog project key, which can
only write events; it is in the release binary by design, and in no file of the repository, log
line, PR or issue.

**The operator's switch.** On by default in a released build, and said plainly where a
person sees it: one sentence with Turn off in the first run, under the wake step's wake-takes
line at the same weight ("Ghost sends an error report when something in it breaks: what failed
and where, never what you say or type."), and Settings › General's Error reports beside
Updates: "Send error reports", what is sent in one sentence, and What was sent, the last 20
reports exactly as sent but for the key (kept 0600 in `~/.local/share/ghost/reports/sent/`).
Off writes `reports/off`, and then nothing is sent, nothing waits, and the install id and the
spool are deleted. The switch is a file both processes read before each report and each send,
so the daemon and the shell need not ask each other. Only the operator reaches it: `GET
/reports` and `PUT /reports/choice` on the control API, and `ghostd reports on|off|status|test`
(`test` sends one report marked `test` and prints it). No tool of Ghost's own app, nothing for
an outside client, nothing on the phone path; no model can turn it on or off or read a report
(`no_tool_reaches_error_reports`, `nothing_an_outside_client_must_not_reach_is_offered`,
`the_phone_has_no_way_to_error_reports`).

**Robust and cheap.** A report goes to the spool (`reports/spool/`, 0700, files 0600): at
most 50 reports or 512 KB, one per fingerprint waiting. A background task in each process
sends the spool at start, when a report is spooled and once a minute, with a 5 s connect and
10 s request timeout; a report is tried three times and then dropped; the same fingerprint goes
once an hour at most and an install sends 30 a day at most (`reports/ledger.json`). A
failure to send is a debug line, never the waiting list, never said. Two processes share the
spool by renaming a report before sending it. Quitting never waits for it.

**Readable frames.** Rust: the release profile keeps line tables (`debug = 1`, as before, for
the journal's panic locations) and the bundle does not strip them, so a backtrace has function
names and `file:line` for Ghost's own frames, kept relative to the workspace
(`crates/ghostd/src/daemon.rs`); a dependency's is `<crate>-<version>/…`, the standard
library's `rustc/…`. Pages: the release bundle is minified and its frames are sent as they
are (`assets/index-<hash>.js:1:2345`, `resolved: false`) with the version; the hash in the
file name names the bundle. Readable page frames need the source maps uploaded to PostHog at
release time (`posthog-cli sourcemap inject` and `upload` in release.yml), which takes a
PostHog personal API key with the error tracking write scope as a second repository secret;
not yet.

**Not yet:** the Android app and the website send no reports.

## What it costs

Nothing on a turn (ADR 0018). A failure is reported after it happened, from a blocking task:
building and writing a report of about 2 KB is under a millisecond of file work, and a
repeat within the hour is dropped in memory before it is built. The panic hook captures a
backtrace (tens of milliseconds, symbolizing from the binary's own line tables) only when the
process is already panicking. The sender is one task per process that wakes once a minute to
list a folder that is almost always empty. Disk: at most 512 KB of spool and 20 kept reports
(about 60 KB). Network: at most 30 requests of a few kilobytes a day.

## Privacy, said honestly

Until now nothing left the machine before the wake word except what the operator started
(a model check, an app) and ADR 0066's update check and speech model download. Now an error
report can: a panic or a failure while Ghost is idle is sent when it happens. It carries what
is listed above and nothing else; it never carries anything heard, said or typed, and the
idle-audio rule is unchanged in what it protects: passive audio and passive transcripts still
go nowhere. A report is not an app, and no agent, model, tool or outside client can make one,
read one, or turn them on.

**Not a bespoke client for a service an agent can reach** (CLAUDE.md's rule on apps). This is
Ghost's own plumbing, like the updater's one GET and the speech-model download: no agent
reaches it and it reads nothing back. The rule names it beside the ntfy route.

## Considered

- **posthog-rs.** It wraps the same capture call with a client, a batch queue and its own
  types for person and group properties; the call is twenty lines and the shape is the point
  of the tests. Not used.
- **posthog-js in the webview.** It is the analytics SDK first (autocapture, page views,
  session replay off only by configuration), and it would be a second sender with its own
  scrubbing in a page that is not trusted to build a report.
- **Only the daemon sends; the shell spools.** The daemon has the vault's redaction, but the
  failures that matter most on a stranger's machine are the daemon not starting at all, and
  then nothing would ever send. Both send; the daemon redacts vault values, and the scrubber's
  token rule covers what the shell cannot know.
- **Hooking the logger and sending every `error!` line.** A log line can carry an app's error
  text. Rejected.
- **Off by default.** The owner's call: on in a released build, said in the first run, one
  switch.
