# Dictation

Ghost can write down what you say and put it on your clipboard: start it, speak, stop it,
then paste with Ctrl+V wherever you want the words. Ghost reads your voice with the same
speech model it uses for conversations, on this computer. Nothing you dictate is sent
anywhere or kept by Ghost ([ADR 0077](adr/0077-dictation-the-operator-speaks-and-ghost-writes-it-down.md)).

This is the first version: the text goes to the clipboard, and you paste it yourself.
Holding a key to talk and pasting for you come later.

## Try it from a terminal

```sh
ghostd dictate start     # the microphone opens; speak
ghostd dictate stop      # Ghost reads it and puts it on the clipboard
```

Then press Ctrl+V (Ctrl+Shift+V in a terminal). `ghostd dictate toggle` starts when nothing
is going on and stops when Ghost is recording. `ghostd dictate cancel` drops it: nothing
reaches the clipboard. `ghostd dictate status` says where it is and how the last one ended.

## Bind it to a key (GNOME)

1. Open **Settings › Keyboard › View and Customize Shortcuts › Custom Shortcuts**, and add
   one.
2. Name: `Ghost dictation`. Command: `ghostd dictate toggle` (with the AppImage, run
   `ghostd command install` once so `ghostd` is on your PATH; or give the full path).
3. Set a key that nothing else uses. Pick a spare one: F9 if you are not using it for
   another dictation tool.

Press the key, speak, press it again. A notification says "Dictating" while Ghost records,
then how many characters are on your clipboard. It never shows the words.

On Hyprland or Sway, bind the same command in your compositor's config, for example
`bind = , F8, exec, ghostd dictate toggle`.

## What to expect

- **Up to 30 seconds** a time. At 30 seconds Ghost stops by itself and writes down what you
  said.
- **Fillers go**: "um", "uh", "er", "hmm". A word said three times in a row ("I I I") is kept
  once. Line breaks, tabs and control characters never reach the clipboard, so pasting into
  a terminal never runs a command.
- **The clipboard is replaced.** What you had copied before is gone; Ghost doesn't put it
  back yet.
- **While dictating, Ghost doesn't listen for its name**, and nothing you dictate wakes it or
  is kept as a wake recording. It listens again as soon as you stop.

## When it says no

| Ghost says | Why |
|---|---|
| Ghost isn't listening. Turn listening on to dictate. | You turned listening off. Dictation keeps that promise: no microphone. |
| Ghost is in a conversation. Dictate when it ends. | A conversation, a phone call, or a typed reply is going on. |
| The Hearing test has the microphone. Finish it first. | |
| Dictation needs Ghost's speech model, which isn't ready yet. | The speech model is still downloading or loading. |
| Ghost can't find a microphone. / Ghost couldn't open the microphone. | Check Settings › Sound. |
| Ghost couldn't reach the clipboard: there is no display. | Ghost runs without a desktop (the headless service with no `DISPLAY`). |
| Too long for the clipboard in one go. | More than 16 KiB of text, far past 30 seconds of speech. |

Turning listening off, starting a conversation, or quitting Ghost while you dictate cancels
the dictation, and nothing reaches the clipboard.

## Where your words go

Only to the clipboard. Ghost doesn't store them, log them (not even in debug mode), show them
in the app or on the phone, or give them to a model, an agent, a tool or an outside app. No
tool of Ghost's can start dictation; only you can, from your keyboard or your terminal.

Once the words are on the clipboard, they are your desktop's: GNOME keeps a copy of the
clipboard, a clipboard manager you installed may keep a history, and the app you paste into
may save or send what you paste.
