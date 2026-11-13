# The realtime voice

Ghost's voice runs on this computer by default: Parakeet hears you, your model connection
answers, Kokoro speaks, and your voice never leaves the machine. You can choose a realtime
voice instead, in **Settings › Sound › Voice mode**. It answers within a moment, like the
voice assistants on phones, and hands every real request to Ghost's agent. It is off until
you turn it on, and Ghost never turns it on by itself.

## What changes when it is on

Settings shows this before it is on:

- After the wake, for the whole session, your voice itself goes to OpenAI Realtime: it hears
  you and answers in its own voice.
- Before the wake nothing changes: the wake word and everything Ghost hears while it waits
  stay on this machine. The phone's calls, typed turns and scheduled runs keep the voice on
  this machine.
- Ghost's agent still does every lookup and every change, under your permissions. The
  realtime voice only talks: it hands your requests to the agent and says its answers word
  for word, and Ghost checks what it says.
- It costs about 1 to 5 US cents a minute of conversation with the mini model and 5 to 15
  with the full one, growing as a session gets long, on top of what the model connection
  already costs, billed by OpenAI Realtime to the key you give it. You can set a daily cap,
  after which Ghost uses the voice on this machine.
- You can switch back to the voice on this machine at any time. Ghost never switches it on
  by itself, and nothing an app, a tool or a model says can.

A few more things to know:

- **On hold** ("hang on, Ghost") nothing goes to the realtime voice. **While Ghost speaks**,
  what the microphone hears is kept back on this machine and sent only if you are talking
  over Ghost.
- **Your yes to a change** is heard by Ghost on this machine, never by the realtime voice,
  and the realtime voice doesn't answer on its own while a change waits for it.
- **What is said is checked.** If the realtime voice starts saying something other than the
  agent's answer, Ghost stops it and says the rest in its own voice.
- **"Ghost, stop listening"**, "hang on", a goodbye and Ghost's other quick phrases work as
  before. With listening off no realtime session opens.
- Ghost keeps none of the session's audio. The transcript keeps your words as Ghost heard
  them and what was said back.
- An agent's own voice applies on this machine only: in a realtime session every agent speaks
  in the realtime voice.

## Turning it on

1. Get an API key from the OpenAI platform and save it in **Settings › Passwords** as
   `OPENAI_API_KEY` (or under another name and set that name in Voice mode).
2. In **Settings › Sound › Voice mode**, choose **Realtime** and read what changes.
3. Press **Test**: Ghost connects, has it say one line and hands it one request, and shows
   how long the first sound took and whether the request reached the agent. It costs a
   fraction of a cent.
4. Optionally set a **daily cap** in US dollars.

The next session uses it. Choose **On this machine** to go back; the session after uses
your own voice again.

## When it goes wrong

If the realtime voice can't be reached, refuses the key, asks Ghost to slow down or drops in
the middle of a session, Ghost says once "I lost the realtime voice, so I'm answering on this
machine for now.", finishes what it was saying in its own voice, and goes on with the voice on
this machine for the rest of that session. A note on the waiting list says what happened; it
is shown, never said. The next session tries the realtime voice again. A missing password or
the day's cap reached starts the session on this machine with the note and nothing said.

## What it costs

Settings shows today's spend and the last session's, estimated from the provider's own counts
at its prices as Ghost knows them; the provider's bill is the truth. Each answer the agent
gives is spoken by the realtime voice, so a long answer costs its audio.

## For the curious

The settings, all in `settings.env` and none of them a password:

```text
GHOST_VOICE_MODE               local (the default) or realtime
GHOST_REALTIME_MODEL           gpt-realtime-2.1-mini (the default) or gpt-realtime-2.1
GHOST_REALTIME_VOICE           marin (the default), cedar, alloy, ash, ballad, coral, echo, sage, shimmer, verse
GHOST_REALTIME_PASSWORD        the password's name, OPENAI_API_KEY by default
GHOST_REALTIME_DAILY_CAP_USD   the day's cap; empty for none
GHOST_REALTIME_SILENCE_MS      how long a pause ends your turn for the realtime voice (500)
```

`ghostd llm test --realtime` runs the same Test from a terminal. To time it against the real
provider with a key you hold in your shell (never the vault, never printed):

```sh
OPENAI_REALTIME_TEST_KEY=sk-... make perf-turns REALTIME_KEY_ENV=OPENAI_REALTIME_TEST_KEY
```

The design is [ADR 0063](adr/0063-a-realtime-voice-the-operator-may-choose.md).
