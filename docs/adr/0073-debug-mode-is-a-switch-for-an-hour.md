# 0073: Debug mode is a switch for an hour

- **Status:** accepted
- **Date:** 2026-11-14
- **Amends:** the idle-audio rule in CLAUDE.md ("The one exception is `GHOST_DEBUG_AUDIO=1`"),
  ADR 0010 (passive transcripts are discarded), ADR 0072 (Developer holds the switch)

The rule was: `GHOST_DEBUG_AUDIO=1`, off by default, is set by the operator by hand for a
debugging session; it logs transcripts and audio decisions to the local journal or terminal
only, through `ghost_voice::debug`; never turn it on in a committed config or unit file. With a
released build on a laptop there is no hand-started session: Ghost runs inside the app, started
by the desktop or at login. The owner asked on 2026-11-14 for "the toggle for debug mode with
the auto off", to test release builds on a laptop and share what they find.

## Decision

**A switch in the daemon's memory, on for one hour.** Settings › Developer's Debug mode and
`ghostd debug-audio on|off|status` turn on exactly what `GHOST_DEBUG_AUDIO=1` turns on, through
the same gate (`ghost_voice::debug::enable`): what Ghost hears and its audio decisions, logged by
`debug::audio` to the daemon's own log (the journal or the terminal), nothing more and nowhere
else. The pipeline is unchanged; the switch is `ghostd::debug_audio`.

- **Never persisted.** It is not written to `settings.env`, the store or any file Ghost reads at
  its start, so a restart of Ghost always turns it off.
- **Off by itself.** A timer in the daemon, not the page, turns it off one hour after it was
  turned on; turning it on again starts a new hour. Developer shows the time left and **Turn
  off now**.
- **Unmistakable while on.** The status carries its end (`Status.debug_audio`); the rail's
  status card wears a "Debug" pill with the minutes left, linking to Developer; the floating
  view wears "Debug mode: logging what it hears" as it wears "Screen shared"; the waiting list
  carries one note (`debug:audio`, its place Developer) saying so with the time it ends.
- **One bounded copy, for a diagnostic report.** While the switch is on, the same lines are
  also kept in `debug-audio.log` under Ghost's data folder (0600, at most 4 MB, redacted against
  the vault as every log line is), so the person can tick "Add the debug log" on About's
  diagnostic report (ADR 0072). The file is deleted when debug mode turns off, by the timer, by
  Turn off now or by `ghostd debug-audio off`, and any left by a Ghost that stopped while it was
  on is deleted at the next start. A report saved while it was on carries a copy; nothing else
  does. The lines never reach the desktop's log mirror or a report's log tail.
- **The operator's only.** The control API's `PUT /debug-audio` (the desktop's Developer page)
  and `ghostd debug-audio`, through the daemon's socket. No tool of Ghost's own app, run, outside
  client, phone route, app's text or model can turn it on: closed-door tests on Ghost's own tools,
  the outside listener's and the phone's routes hold it.
- **Listening off changes nothing.** With listening off (ADR 0057) no capture is open, and the
  switch opens none: it is a log flag; nothing is heard, so nothing is logged.
- **The environment variable stays as it was.** A Ghost started with `GHOST_DEBUG_AUDIO=1` logs
  until that process ends; the switch shows it on, from the environment, and cannot turn that
  off.

## What it does not allow

- Turning it on for longer than an hour at a time, or across a restart.
- Logging anything `debug::audio` does not log today, or sending its lines anywhere but the
  daemon's log and, while on, the bounded file above.
- Keeping passive audio, or any audio: it logs words and decisions, as the variable does.
- Any path for a model, tool, run, app, outside client or the phone to turn it on, read its
  file, or keep it on.
- Writing `GHOST_DEBUG_AUDIO` into `settings.env`, a unit or a committed config: the experimental
  switches (ADR 0072) refuse it by name.

## Consequences

- While on, passive transcripts are in the daemon's log for that hour, as under the variable,
  and in the 0600 file until it turns off. The rule in CLAUDE.md says so.
- `ghost_voice::debug` keeps its one public gate; a test of the switch shares that process-wide
  flag, so its tests run one at a time.
