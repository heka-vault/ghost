# Dictation

Hold a key, speak, let go: Ghost types what you said where your cursor is. Ghost reads your
voice with the same speech model it uses for conversations, on this computer, and you see
your words beside your work as you speak
([ADR 0077](adr/0077-dictation-the-operator-speaks-and-ghost-writes-it-down.md)).

## Turn it on

Open **Settings › Sound** and turn on **Hold a key to dictate**. GNOME asks twice, once each:

1. to let Ghost use a key (F9, or the first of F10, F8, F7 and F6 that nothing else uses;
   pick another in GNOME's dialog if you like), and
2. to let Ghost type for you.

After that it just works, at every start. **Change key** opens GNOME's own place for it.

## Use it

- **Hold** the key while you speak, and let go when you're done.
- Or **tap** it: recording stays on until you tap again. Handy for something long.

Ghost appears over your desktop while it listens (the floating room, Settings › General).
It shows your words as it reads them, a phrase at each pause, then types the whole text once
into the window that has the keyboard and goes away. If Ghost's own window has the keyboard,
its Today page shows the same.

## What to expect

- **Up to two minutes** a time. At two minutes Ghost stops by itself and types what you said.
- **Tidied by a model**, by your choice under **Tidy with** (Settings › Sound › Dictation):
  misheard words, punctuation and capitals are fixed before typing. If the model takes more
  than three seconds, or changes more than small fixes, Ghost types what it heard. The choices:
  - **Off**: Ghost types what it heard.
  - **Claude Code · Haiku, Sonnet or Opus**: your own Claude Code with that model (offered
    while it is Ghost's model or on this computer). About 3 seconds, most of it the program
    starting, so some dictations come back untidied.
  - **OpenRouter** · Claude Haiku 5.5 (`anthropic/claude-haiku-5.5`), Gemini 3.5 Flash Lite
    (`google/gemini-3.5-flash-lite`), Llama 3.1 8B on the fastest host
    (`meta-llama/llama-3.1-8b-instruct:nitro`), or another model id you type: under a second.
    These need your OpenRouter key, added right there (it goes into Ghost's vault and never
    comes back out); until it is, they are shown but can't be chosen.

  A Ghost set up before there was a choice keeps doing what it did: OpenRouter when its key is
  set, otherwise Ghost's own model (shown as "Ghost's model"), and Off if the tidy was off.
- **Fillers go**: "um", "uh", "er", "hmm". A word said three times in a row ("I I I") is kept
  once. Line breaks, tabs and control characters are never typed: no Enter is ever sent, so
  typing into a terminal never runs a command.
- **Your speakers are muted** while you dictate, so music doesn't get into the microphone, and
  come back when you stop.
- **English only.** Ghost reads your voice as English: its speech model chooses English
  letters only, so a misheard sound never comes out as a Cyrillic or accented letter. What it
  types is plain English characters too, because GNOME can type only the keys your keyboard
  has: an accent comes off (café is typed cafe, ½ as 1/2), curly quotes, long dashes and "…"
  become their plain forms, and anything else (another script, an emoji) is left out. Only
  when nothing typable is left does the text go to your clipboard ("Press Ctrl+V").
- **While Ghost isn't listening**, the key still works: pressing it is you opening the
  microphone, for that dictation only. Nothing else does.
- **While dictating, Ghost doesn't listen for its name**, and nothing you dictate wakes it or
  is kept as a wake recording. It listens again as soon as you stop.
- **Not during a conversation** yet: the key is refused until the conversation ends.

## Without the key: a command, or a desktop of your own

Where the desktop has no way for apps to use a key (X11, Hyprland, Sway), make a keyboard
shortcut of your own that runs:

```sh
ghostd dictate toggle    # starts when nothing is going on, stops when Ghost is recording
```

On GNOME: **Settings › Keyboard › View and Customize Shortcuts › Custom Shortcuts**. On
Hyprland: `bind = , F8, exec, ghostd dictate toggle`. With the AppImage, run `ghostd command
install` once so `ghostd` is on your PATH. A dictation from a command ends on the **clipboard**
(press Ctrl+V; Ctrl+Shift+V in a terminal), never typed, and is refused while Ghost isn't
listening.

From a terminal: `ghostd dictate start`, `stop`, `cancel` (nothing is kept), and `status`
(`Dictating: …` while Ghost records, `Writing it down…` while it reads, and when nothing is
going on, `Not dictating.` and how the last one ended). A script can read the state from the
control API, `GET /dictation`.

## When it says no

| Ghost says | Why |
|---|---|
| Ghost isn't listening. Turn listening on to dictate. | A command while listening is off. The key still works. |
| Ghost is in a conversation. Dictate when it ends. | A conversation, a phone call, or a typed reply is going on, or a microphone Ghost had open is still closing (it waits up to 3 seconds). |
| The Hearing test has the microphone. Finish it first. | |
| Dictation needs Ghost's speech model, which isn't ready yet. | The speech model is still downloading or loading. |
| Ghost can't find a microphone. / Ghost couldn't open the microphone. | Check Settings › Sound. |
| Some characters can't be typed here. On your clipboard: … Press Ctrl+V. | Nothing in it could be typed (only another script or emoji): see above. |
| Ghost couldn't type it. On your clipboard: … Press Ctrl+V. | Typing isn't allowed (turn the switch off and on to be asked again) or failed. |
| Ghost couldn't reach the clipboard: there is no display. | Ghost runs without a desktop (the headless service with no `DISPLAY`). |

Turning listening off, starting a conversation, or quitting Ghost while you dictate cancels
the dictation, and nothing is typed or put on the clipboard.

Ghost listens for its name again only once dictation's microphone has closed. If it ever
doesn't, the waiting list says "Dictation's microphone didn't close" until it does, and
quitting Ghost always works.

## Where your words go

- **To the window you're typing in**, or the clipboard when they can't be typed.
- **To your own screen**, as you speak: Ghost's floating room or its window shows them, and
  keeps nothing.
- **To the model you chose under Tidy with**, unless it is Off. Settings › Sound says where in
  one line under it ("Your words go to Anthropic through your Claude Code, about 3 s." or
  "Your words go to OpenRouter, under a second."): your dictated words leave this computer to be
  tidied. Choose Off to keep them here.

Ghost doesn't store them, log them (not even in debug mode), send them to the phone, or give
them to an agent, a tool or an outside app. No tool of Ghost's can start dictation; only you
can, with the key or your terminal.

Once the words are in a window or on the clipboard, they are that app's and your desktop's:
GNOME keeps a copy of the clipboard, a clipboard manager may keep a history, and the app you
type into may save or send what you type.
