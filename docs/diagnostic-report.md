# The diagnostic report

A diagnostic report is one file you save and hand to whoever is helping you with Ghost
(ADR 0073). Ghost writes it; it never sends it. Save one from **Settings › About › Save a
diagnostic report**, or in a terminal:

```
ghostd diagnose [--out FILE] [--transcripts] [--debug-log] [--wake-recordings]
```

It goes to your Downloads folder (the one your desktop names in `~/.config/user-dirs.dirs`,
else `~/Downloads`, else your home folder) as `ghost-diagnostic-<date>-<time>.json`, readable
only by you (0600), never over a file already there. With `--out`, it goes where you say.

Before Settings saves it, you see each part it will hold and its size; the parts with your own
words or voice are marked, and are there only if you ticked their boxes.

## What it holds

Every text field below passes the same scrubbing as an error report
([error-reports.md](error-reports.md)): Ghost's vault redacts every password value first, then a
web address keeps only its scheme, host and path, an email address becomes `[email]`, your home
folder becomes `~`, your user and computer names become `[user]` and `[host]`, an IP address
becomes `[ip]`, and a long run of letters and digits that looks like a key becomes `[token]`. A
log line, an error and a problem keep only the code's own words: what follows the first `": "`
(usually someone else's text, such as an app's error) is dropped, and anything in quotes emptied.

| Field | What it is |
| --- | --- |
| `ghost_diagnostic_report` | The format, `1`. |
| `about_this_file`, `saved_at` | A sentence on what the file is, and when it was saved. |
| `version`, `build` | Ghost's version and how it was built (release, deploy or a checkout, and its commit). |
| `computer` | The OS and its version, X11 or Wayland, the processor type, and .deb, AppImage or checkout. |
| `desktop` | The app's own lines: this window's version, whether Ghost can float and why not, and the updater's state. |
| `display` | Whether an X server is reachable (Ghost needs X11 or XWayland to float) and whether the session is Wayland. |
| `sound` | The microphone setting, the speakers, how Ghost hears you (on the microphone's own chip, a headset, Ghost's echo canceller, or unprotected; `tested_array` when it is a microphone array that cancels Ghost's voice itself), what a session and the wake detector listen to, the microphone's problem if any, whether the echo canceller is installed and running, and the sound devices by name with their channels and rate. |
| `model` | The provider and model names (and the fast model's), and the last model check: when, each model's first byte and first word, what works (streaming, tools, caching, thinking), and its advice. Never a key or an address. |
| `speech_models` | Whether Parakeet and Kokoro are loaded, how many files are missing and their size, free space, and the download's state. |
| `turn_timings` | The last 30 turns' timing records from `latency.jsonl`, numbers, yes/no and times only: model ids, tool names and anything else in words are dropped. |
| `wake_attempts` | How many wake attempts are kept, how many woke and how many were refused, whether keeping is on, and the last 50 as time, woke or not, score, threshold and whether labelled. Never their sound or what Parakeet read. |
| `error_reports` | The last error reports this install sent, exactly as they were sent (already scrubbed when made). |
| `counts` | The apps by name and state (connected, needs a sign-in…), how many agents and schedules. Never what an app returned. |
| `log` | Ghost's last 400 log lines, scrubbed as above. Debug mode's lines are never in them. |

## What you can add

Three boxes, all unticked, each with what it adds:

| Box | Field | What it is |
| --- | --- | --- |
| Add recent transcripts (`--transcripts`) | `added_by_you.transcripts` | The last 40 turns of your conversation with Ghost: your words and its answers, word for word (passwords redacted, as the store keeps them). |
| Add the debug log (`--debug-log`) | `added_by_you.debug_log` | While debug mode is on only (ADR 0074): every line it logged, what Ghost heard word for word and its audio decisions. Turn debug mode on in Settings › Developer, do what goes wrong, then save the report before the hour is up. |
| Add kept wake recordings (`--wake-recordings`) | `added_by_you.wake_recordings` | Up to the eight newest kept wake takes as WAV files (base64), your voice saying Ghost's name with a few seconds before it. |

## What it never holds

What you said or typed, unless you add transcripts; audio, unless you add wake recordings;
a password; an app's content or its error text; an agent's persona or memory; your home path;
an address's user name, password or query. `crates/ghostd/src/diagnose.rs`'s tests feed each of
these in and check the file.

No tool of Ghost's own, no agent's run, no outside tool, the phone, an app or a model can save a
report or read one. It is the operator's: `POST /diagnose` on the control API and
`ghostd diagnose`.
