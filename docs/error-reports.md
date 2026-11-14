# Error reports

When something in an installed Ghost breaks, Ghost sends an **error report** so it can be fixed:
what failed, where in Ghost's code, the version, and a few coarse facts about the computer. It
never sends what you said, typed or heard. The decision is [ADR 0067](adr/0067-ghost-sends-error-reports-from-a-released-build.md).

## When a report is sent

Only from a released build (the .deb or the AppImage from a GitHub release), and only while
**Send error reports** is on (Settings › General; on by default, and said in the first run). A
build from the checkout never sends one. A report is sent for:

- Ghost's daemon or the app crashing (a Rust panic), in any of its processes;
- a page of the app failing (the main window or the floating room);
- one of a short list of times Ghost did not work: a turn's model call failed, the speech
  models would not load or download, the sound output stopped playing, the microphone stopped
  hearing well, an update did not install, the app could not start the daemon, or the daemon
  exited while the app ran it.

The same failure goes at most once an hour, and at most 30 reports a day.

## What a report carries

| Field | What it is |
| --- | --- |
| `event` | Always `$exception`. |
| `distinct_id` | A random id made for error reports on this computer; deleted when you turn them off. |
| `timestamp` | When it happened. |
| `$exception_list` | The error: its type, its message after scrubbing (or nothing, when the message is not Ghost's own), and the stack: function names and `file:line` in Ghost's code. |
| `$exception_fingerprint` | A hash that groups the same failure. |
| `$process_person_profile`, `$geoip_disable` | `false` and `true`: no profile is made of you and the address the request came from is not looked up for a place. |
| `ghost_kind` | `panic`, `page`, `failure` or `test`. |
| `ghost_version`, `$lib_version` | Ghost's version. |
| `ghost_component` | `daemon`, `shell`, `floating` or `page`. |
| `ghost_facts` | A few words about the failure: where a panic was, which page, the HTTP status's class (`5xx`), the model's name, how the daemon exited. |
| `os_name`, `os_version` | From `/etc/os-release`: `Ubuntu`, `24.04`. |
| `session_type` | `x11`, `wayland`, `tty` or `unknown`. |
| `arch` | `x86_64`, `aarch64`. |
| `installed_as` | `deb`, `appimage` or `checkout`. |

## What a report never carries

What you said, typed or heard; audio; an app's words (descriptions, results, errors); a
model's or a provider's words; a password; your name, your computer's name, your email, the
paths in your home folder (they become `~`); a web address's query; an IP address; the
contents of your files in `~/.config/ghost`; an agent's name or persona. Every message passes
fixed rules before it is kept or sent, and a message that is not from Ghost's own code is
reduced to its type.

## An example, exactly as sent

```json
{
  "api_key": "(Ghost's project key)",
  "event": "$exception",
  "distinct_id": "5b0f3c1e-8d2a-4e57-9a61-2c7d40e8b913",
  "timestamp": "2026-11-14T13:34:48.434Z",
  "properties": {
    "$exception_list": [
      {
        "type": "panic",
        "value": "index out of bounds",
        "mechanism": { "handled": false, "synthetic": false, "type": "generic" },
        "stacktrace": {
          "type": "raw",
          "frames": [
            { "platform": "custom", "lang": "rust", "function": "ghostd::session::Session::run::{{closure}}",
              "filename": "crates/ghostd/src/session.rs", "lineno": 288, "colno": 14, "in_app": true, "resolved": true },
            { "platform": "custom", "lang": "rust", "function": "ghost_voice::tts::Tts::clause",
              "filename": "crates/ghost-voice/src/tts.rs", "lineno": 412, "colno": 27, "in_app": true, "resolved": true }
          ]
        }
      }
    ],
    "$exception_fingerprint": "9c41d07a2be35f18",
    "$process_person_profile": false,
    "$geoip_disable": true,
    "$lib": "ghost-reports",
    "$lib_version": "2026.11.1",
    "ghost_kind": "panic",
    "ghost_version": "2026.11.1",
    "ghost_component": "daemon",
    "ghost_facts": { "location": "crates/ghost-voice/src/tts.rs:412:27", "thread": "tokio-runtime-worker" },
    "os_name": "Ubuntu",
    "os_version": "24.04",
    "session_type": "wayland",
    "arch": "x86_64",
    "installed_as": "deb"
  }
}
```

The real request carries Ghost's project key where this shows `(Ghost's project key)`: a key
that can only send events to Ghost's project, the same in every copy of a release.

## Turning it off, and seeing what was sent

- **Settings › General › Error reports**: the switch, and **What was sent**: the last 20
  reports this computer sent, exactly as above.
- `ghostd reports off` (and `on`, `status`, `test`: `test` sends one report marked as a test
  and prints it).

Off means nothing is sent, nothing is kept waiting, and the random id is deleted; turning it
on again makes a new one. No agent, model, app, outside tool or the phone can turn reports on
or off or read them.

## Where it is kept

`~/.local/share/ghost/reports/`: `install-id`, `spool/` (reports waiting to be sent, at most 50
or 512 KB), `sent/` (the last 20 sent), `ledger.json` (the day's count and the last hour's
fingerprints) and `off` when you turned it off. Every file is readable by you only. Deleting
the folder (with Ghost stopped) forgets all of it.
