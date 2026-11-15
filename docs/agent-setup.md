# Set up Ghost: a guide for the person's coding agent

You are a coding agent (Claude Code, Codex, Cursor or another) on a person's Linux computer.
They asked you to set up Ghost, a voice assistant that runs on their desktop
(<https://heyghost.app>, source at <https://github.com/heka-vault/ghost>). Read all of this
before you run anything. Do the steps in order. Each step gives the exact command, run as the
person's own user, and what a correct result looks like. Each code block is one shell command:
run it whole, since a later block may not see an earlier block's variables.

## Never do these

1. Never read, copy, move or type any password, API key or token into Ghost, a file or a
   command line. That includes your own sign-in or subscription: it is licensed for you only.
   Never ask the person to paste a key into this chat.
2. Never change Ghost's permissions, `~/.config/ghost/` or `~/.local/share/ghost/` by hand.
3. Never turn on the microphone, listening, the realtime voice, any error-report choice or
   wake-take keeping for the person, and never play a sound or open the microphone yourself.
4. Never run `curl … | sh`, add an apt repository, or edit a system file. Never type or see
   the person's login password.
5. If something is not in this guide, or a result is not what it says, stop and ask.

## 1. Tell the person what Ghost is, and get a yes

Say this in your own words, then ask whether to go ahead. Do nothing else until they say yes.

- Ghost listens for its name ("Hey Ghost") while it runs. Before it hears its name, the
  microphone's audio goes only to speech models on this computer's processor and is thrown
  away. Listening can be switched off at any time.
- After the wake word, their words go as text to the model provider they choose (Anthropic,
  OpenRouter, OpenAI), or nowhere with a local model (Ollama, LM Studio).
- It acts through the apps they connect (GitHub, a calendar, …): by default it does what they
  ask and says what it did; Settings › Permissions sets what it must ask about first.
- It keeps conversation transcripts and an audit log of every change on this computer, under
  `~/.local/share/ghost`; none of it is uploaded by Ghost.
- When something in Ghost breaks, it sends an error report: what failed and where in its
  code, never what they said or typed. On by default; Settings › General turns it off.
- Early software, Linux only, tested with one microphone only (a reSpeaker XVF3800 USB array).
- The download is about 180 MB (the .deb; 240 MB for the AppImage), and its ears and voice are
  2.9 GB more, downloaded once. Installed it takes about 4 GB of disk.
- Installing the .deb needs their password once, typed by them in their own terminal.

## 2. Check the computer

```bash
uname -sm; . /etc/os-release; echo "$ID $VERSION_ID"
echo "session: ${XDG_SESSION_TYPE:-none} ${WAYLAND_DISPLAY:-} ${DISPLAY:-}"
pactl info 2>/dev/null | grep '^Server Name' || ps -C pipewire,pulseaudio -o comm= || echo "no sound server"
df -h --output=target,avail "$HOME" /tmp
command -v ghostd heka-ghost-desktop || echo "Ghost: not installed"
```

Read the result:

- **`Linux x86_64`** is required. Anything else: stop and tell the person Ghost runs on x86-64
  Linux only.
- **`ubuntu 24.04` or `ubuntu 26.04`**: use the .deb (steps 3 to 7). Another x86-64 Linux: the
  AppImage (step 3 with `.AppImage`, then the note at the end of step 4). The AppImage needs
  FUSE 2 (`libfuse2t64` on Ubuntu); without it it runs with `--appimage-extract-and-run`.
- **The session is `x11` or `wayland`** with a display set. `none` (an SSH login, a container):
  stop; Ghost is a desktop app and needs the person at their desktop.
- **Sound**: `PulseAudio (on PipeWire …)` or a `pipewire` process is best. Plain PulseAudio
  works, but Ghost's echo canceller can't be set up there: suggest headphones.
- **Space**: at least 4 GB free in `$HOME` and 500 MB in `/tmp`. Less: stop and say so.
- **Already installed** (`/usr/bin/ghostd` printed): stop. Tell the person Ghost is here, and
  that Settings › General › Updates checks for a new version. Run `ghostd version` and say it.

## 3. Download the latest release and verify it

No version is typed here: GitHub's API names the latest release, its file and its SHA-256.

```bash
dir=$(mktemp -d) && chmod 755 "$dir" && cd "$dir" &&
curl -fsSL https://api.github.com/repos/heka-vault/ghost/releases/latest -o release.json &&
read -r tag name size sha url < <(python3 -I -c '
import json, sys
r = json.load(open(sys.argv[1]))
a = next(a for a in r["assets"] if a["name"].endswith(sys.argv[2]))
print(r["tag_name"], a["name"], a["size"], a["digest"].removeprefix("sha256:"), a["browser_download_url"])
' release.json _amd64.deb) &&
case "$url" in https://github.com/heka-vault/ghost/releases/download/*) ;; *) echo "unexpected URL: $url"; false;; esac &&
echo "release $tag: $name, $((size / 1000000)) MB" &&
curl -fL --progress-bar -o "$name" "$url" &&
echo "$sha  $name" | sha256sum -c - &&
echo "ready: $dir/$name"
```

For the AppImage, put `_amd64.AppImage` in place of `_amd64.deb`. No `curl`: `wget -qO`
works the same for both downloads.

Correct: `release v…: heka-ghost-desktop_…_amd64.deb, 179 MB`, then `…deb: OK`, then
`ready: /tmp/tmp.…/heka-ghost-desktop_…_amd64.deb`. Keep that path. `OK` proves the file is
byte for byte the one published in the heka-vault/ghost release; it does not prove more than
that GitHub repository's owners made it. Anything other than `OK`: delete the folder, stop and
tell the person. The API allows 60 unauthenticated requests an hour; a `403` means wait.

## 4. Install

Ask the person to run this in their own terminal (fill in the path from step 3), and wait
for them to say it finished:

```bash
sudo apt install /tmp/tmp.XXXXXXXXXX/heka-ghost-desktop_VERSION_amd64.deb
```

apt brings what it needs (WebKitGTK, eSpeak NG, `pulseaudio-utils`). Only if `sudo -n true`
succeeds for you without a password may you run it yourself, after their yes. Then check:

```bash
ghostd version
```

Correct: `Ghost <version> · release <commit>`, the release's tag without its `v`. **Only if it
is `2026.11.0`** (older than this guide): in step 5 use
`grep -v INFO /tmp/ghost-models.log | tail -1` and ask the person not to open Ghost until the
fetch ends; add `> /dev/null` after step 8's `--command` and `sleep 5;` before step 9's `}`.

**AppImage instead**: no password. Move the file from step 3's `ready:` path to
`~/Applications/` and `chmod +x` it. There is no `ghostd` command with the AppImage: skip steps
5 and 6 (the first run does both) and in step 8 use the app's Settings › Outside tools.

## 5. Download Ghost's ears and voice

Parakeet (speech to text) and Kokoro (its voice), 2.9 GB, each checked against its pinned
SHA-256. It takes a few minutes; run it so it outlives your command's time limit, and tell the
person how far it is now and then:

```bash
ghostd models list; nohup ghostd models fetch > /tmp/ghost-models.log 2>&1 &
```

```bash
tail -1 /tmp/ghost-models.log
```

Correct: `missing: …` lines and `ghostd models fetch downloads them (2.9 GB)`, then progress
lines such as ` 41% encoder-model.onnx.data`, ending in `every speech model is here`. Stopped
or failed: run `ghostd models fetch` again; it resumes. The person may open Ghost whenever they
like: the app picks up this same download and shows its progress ("Already downloading").

## 6. Sound

```bash
ghostd echo-cancel status
```

`echo canceller: not running` is normal on a fresh install; `It can't be set up here` says why
(tell the person; suggest headphones). Otherwise ask what they will use. A headset, or a
speakerphone or microphone array with its own echo cancelling: nothing to do. Open speakers
with a laptop or webcam microphone: Ghost's echo canceller keeps it from hearing itself. The
first run offers it (Set it up); you may run `ghostd echo-cancel on` only if they say yes (a
user service, no password; `echo-cancel off` removes it). No sound and no microphone from you.

## 7. A model, and opening Ghost

Ghost needs one model connection. With Claude Code signed in on this computer the first run
offers **Use Claude Code**: Ghost runs their own Claude Code, and replies count against their
Claude plan's limits (not signed in: they run `claude auth login` themselves). Otherwise an
Anthropic, OpenRouter or OpenAI API key, typed by them into Ghost's own window or with
`ghostd passwords set ANTHROPIC_API_KEY` (or `OPENROUTER_API_KEY`, `OPENAI_API_KEY`) in their
own terminal, or a local model in Ollama or LM Studio (no key). Their click and their typing:
never through you or this chat, and never your own credentials. Check for a local model:

```bash
curl -s -m 2 http://127.0.0.1:11434/api/tags >/dev/null && echo "Ollama is running"
curl -s -m 2 http://127.0.0.1:1234/v1/models >/dev/null && echo "LM Studio is running"; true
```

Then ask the person to open **Ghost** from their app menu (Activities, type "Ghost"); don't
start it from your shell, which would give it your environment and keys. They will see
"Welcome to Ghost", then Get started: connecting a model, letting Ghost hear them, saying "Hey
Ghost", a voice, an app and an agent, each skippable. When they say Ghost is open, check:

```bash
ghostd status | grep -E '"(phase|models_ready|version)"'
```

Correct: `"phase": "passive"` (or `"session"`); `"models_ready": true` once step 5 has finished.

### Use what the person already has

Once Ghost is running, `ghostd setup detect` lists, per tool (`claude-code`, `cursor`, `codex`),
the apps that tool uses, whether it already uses Ghost, and a model connection already here;
nothing changes. Show the person the list, then with their yes: `ghostd setup import <tool>
<app>…` (or `--all`), `ghostd setup link <tool>` (let that tool use Ghost; this replaces step
8's by-hand lines when it works), `ghostd setup unlink <tool>`. Each prints what it will do and
asks y/N; pass `--yes` only after the person has seen that and said yes. Ghost never reads
another tool's sign-in.

## 8. Connect yourself to Ghost

Ghost lets tools on this computer reach it as *outside tools*, each with a name and a token
(<https://github.com/heka-vault/ghost/blob/main/docs/connect-ghost.md>). The token is a
password: with the .deb, use `--command`, which keeps it in a file only the person can read and
does not print it. Name yourself: `claude-code`, `codex`, `cursor` (lowercase letters, digits,
hyphens), in place of `claude-code` below.

```bash
ghostd mcp add claude-code --command && ghostd mcp list
```

Correct: `Connected claude-code. Its token is saved in …` and `claude-code  added …  never used`.
Then add Ghost to your own configuration:

- **Claude Code**: `claude mcp add --scope user ghost -- /usr/bin/ghostd mcp serve --client claude-code`
- **Codex**: `codex mcp add ghost -- /usr/bin/ghostd mcp serve --client codex`
- **Cursor**, and any tool that reads an `mcpServers` file: merge this into `~/.cursor/mcp.json`
  (keep the entries already there):
  `{"mcpServers": {"ghost": {"command": "/usr/bin/ghostd", "args": ["mcp", "serve", "--client", "cursor"]}}}`

Your new tools usually appear in your next session. With the AppImage there is no bridge
command: ask the person to use Ghost's Settings › Outside tools › Connect a tool and paste the
line it shows into their own terminal, not this chat.

## 9. Ghost says hello

Ask the person whether Ghost may say one sentence out loud. With a yes, send it through the
bridge with your client name from step 8 (it reaches the running Ghost; no new tools needed):

```bash
{ printf '%s\n' \
  '{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"setup","version":"1"}}}' \
  '{"jsonrpc":"2.0","method":"notifications/initialized"}' \
  '{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"say","arguments":{"text":"Hello, I am Ghost. Say Hey Ghost whenever you want me."}}}'
} | timeout 30 /usr/bin/ghostd mcp serve --client claude-code | grep '"id":2'
```

Read the `status` in the answer and say it plainly:

- `saying it at the desk`: Ghost chimes, says "From Claude Code: Hello, …" and listens for a
  short reply.
- `on the waiting list` with `the operator has not let Ghost speak first at the desk`: this is
  the default. The hello is on Ghost's Today screen. To hear it, the person turns on **Ghost
  asks at the desk** in Settings › Notifications themselves (a chime, the sentence, then Ghost
  listens for their reply); then you may send it once more. Don't change that setting.
- Another reason (`the desk is busy`, `a quiet time`): say it; don't retry in a loop.

## 10. What to try first

Tell the person:

1. Say **"Hey Ghost"** and ask something; "that's all" sends it away. Typing works too.
2. Turn on **the floating room** (Settings › General): Ghost floats beside their work when
  called, with no window.
3. **Connect an app** (Apps › Add an app), such as GitHub or their calendar.

You can now use Ghost's tools: `say`, `notify` (a note on their waiting list), `agents_list`,
`agent_ask`, `status`, `docs_search`. Your changes go through their permissions and the audit
log, under your name.

## Undo

Run `ghostd uninstall --dry-run` and show the person its list: every kind of thing Ghost left,
with sizes, including `~/.config/autostart/heka-ghost-desktop.desktop` and the window's own
storage, `~/.local/share/dev.heka.ghost`. Remove only what they choose. `ghostd uninstall
--keep-data --yes` stops Ghost and takes all but their data and passwords; those go only on
their explicit yes, each warned about (`ghostd uninstall --all`, or answering per kind). It
ends with the one line the person runs (`sudo apt remove heka-ghost-desktop`). If it says
Claude Code is left, run the command it shows (`claude mcp remove --scope user ghost`).

## Never do these (again)

1. No password, key or token through you, and never your own credentials.
2. No change to Ghost's permissions or its folders by hand.
3. Never turn on the microphone, listening, the realtime voice, any error-report choice or
   wake-take keeping for the person; no sound and no microphone from you.
4. No `curl | sh`, no repositories, no system files, no login password.
5. Not in this guide, or not the result it describes: stop and ask the person.
