# Testing the phone's live connection

A ten-minute check, on a real Android phone, of what ADR 0051 added to the phone app: the
connection that runs with the app closed, notifications you can act on, and a call from Ghost.
The wire format is [phone-protocol.md](phone-protocol.md); the app side is described in
`apps/mobile/README.md`, "With the app closed".

## Before you start

- A build of this branch on the phone: `make android-install` with the phone on USB.
- Ghost running at the desk with the phone listener on both addresses
  (`GHOST_PHONE_LISTEN=<home IP>:8765,<tunnel IP>:8765`), and the WireGuard app on the phone set
  up as in ADR 0020.
- A tool connected to Ghost (Settings, Outside tools; see [connect-ghost.md](connect-ghost.md)),
  for example Claude Code. It is the easiest way to put things on your waiting list on demand.
- In `~/.config/ghost/policy.toml`, for the length of the test, let the phone be the way Ghost
  reaches you, and let it call:

  ```toml
  [routes]
  enabled = ["digest", "phone", "call"]
  ```

  Restart Ghost so it reads the change. Put your own routes back when you are done.

If a step fails, `adb logcat -s GhostLink` shows what the connection did (no message contents are
logged).

## 1. Pair (1 min)

Pair the phone from the desktop's QR code as usual. On the last screen tap **Start using Ghost**;
Android asks to allow notifications. Allow.

Expected: Home opens. A quiet notification "Ghost · Connected to <desk> on your home network"
sits in the shade. Settings, under "Questions and calls with the app closed", says "Connected over
your home network at <home address>", Notifications "On", and Calls over the lock screen
"Allowed" (on Android 14 and later it may say "Not allowed": tap **Allow** and turn it on).

## 2. Close the app (30 s)

Open the recent apps and swipe Ghost away.

Expected: the "Connected to <desk>" notification stays. Nothing else changes.

## 3. A notification from the desk (1 min)

From the connected tool, ask it to call Ghost's `notify` with the title "Test from the desk".

Expected: within about 30 seconds a notification titled with the tool's name and the text "Test
from the desk", with **Got it** and **Later**. Tap **Got it**. The buttons go and it says
"Sending…", then the notification disappears. At the desk, the item shows as heard. Nothing was
shown on the lock screen but "Something is waiting for you".

## 4. Answer a question from the notification (2 min)

From the tool, ask it to call Ghost's `ask` with the question "Ship the build today?" and the
choices Yes and No.

Expected: a heads-up notification "<tool> asks" with "Ship the build today?", and **Answer**,
**Yes**, **No**. Tap **Answer**, type "after lunch", send. The notification says "Sending…" and
then disappears. At the desk, the question shows as answered from the phone with the words
"after lunch", exactly as typed. Opening the app, the question is no longer on Home.

Repeat with a new question and tap **Yes**: the same, with the answer "Yes".

## 5. Confirm a change (2 min)

Make a change wait for your yes: for example ask an agent, through the tool's `agent_ask`, to do
something your permissions set to `ask`, such as labelling an issue in the mock GitHub. The
change waits on the waiting list.

Expected: a notification "<agent> wants your yes" with the change, and **Yes** and **No**. Tap
**Yes**. The notification disappears once the desk has done it. The audit log at the desk shows
the change confirmed with `classifier: phone`. Open the app: Home no longer lists it. (Trying
**No** on another one cancels it, and nothing runs.)

## 6. A call from Ghost (2 min)

Lock the phone. From the tool, `ask` a question with urgency `call`, for example "Can I send the
invoice now?".

Expected: the phone lights up with a full-screen call "<agent> is calling through Ghost", ringing
with your ringtone (or vibrating, or silent, as the ringer switch says). It says nothing about
the question. Tap **Answer**. The phone asks you to unlock; unlock. Ghost opens in a session with
that agent, and Ghost puts the question in the asker's words. Answer by voice, then **End**.

Do it again and tap **Decline** instead. Expected: the ringing stops at once; at the desk the
item is snoozed for 30 minutes. Do it a third time and let it ring: after 30 seconds it stops by
itself.

## 7. Drop the tunnel and bring it back (2 min)

Leave the home Wi-Fi (turn Wi-Fi off) with WireGuard on.

Expected: within a few seconds the shade says "Connected to <desk> through your tunnel", and
Settings shows the tunnel address.

Turn WireGuard off. Expected: within about 35 seconds the shade says "Reconnecting to <desk>"
and then "Can't reach <desk>. Trying again." Send another `notify` from the tool: nothing
arrives on the phone, and the desk moves the item on (to the digest here). Open the app: Home
shows the list with a line saying how old it is and that the phone can't reach the desk.

Turn WireGuard back on. Expected: within a few seconds "Connected to <desk> through your
tunnel" again, and Home's list catches up with no stale line.

## Optional: restart the phone

Settings, turn on **Start when the phone starts**, restart the phone and unlock it.

Expected: the "Connected to <desk>" notification comes back without opening the app. With the
switch off it does not, until you open Ghost.
