# 0070: A yes or a no can stick, as one rule the operator's click writes

- **Status:** accepted
- **Date:** 2026-11-14
- **Amends:** ADR 0034 (the operator sets a rule at the moment of deciding, not only in
  Settings › Permissions), ADR 0035 ("Ghost's folder is the operator's": one more writer of
  `policy.toml`, the operator's click)
- **Builds on:** ADR 0007, ADR 0011, ADR 0013 (a click in the desktop session is evidence),
  ADR 0015 (no undo), ADR 0017 (the words come from the tool's name), ADR 0030, ADR 0047,
  ADR 0051

A review of the best assistant apps (approved by the owner, 2026-11-14) found that people never
write permission rules ahead of time. The good products let the permission be set at the moment
of deciding: Raycast's approval card offers Allow, Always allow and Deny, and its settings page
is where what was allowed is reviewed and removed; OpenAI's dots names a rule's levels in plain
words. Ghost asked for a yes with no way to say "and don't ask me again", so the only way to
stop being asked was to learn Settings › Permissions: a mode, four levels, rules in two forms
and a file. The owner's goal: nine people in ten never visit Permissions.

## Decision

**The card that asks for a yes can make the decision stick.** Beside Confirm and Cancel, a
quiet disclosure, "More choices", opens two more: **"Yes, and don't ask again for “issue
write” in github"** and **"No, and never allow “issue write” in github"**. Neither is the
default action and neither is reached by Enter alone: the disclosure must be opened first.
The confirmation says once, "You can change this in Settings › Permissions"; there is no undo
(ADR 0015), a rule set by mistake is removed there.

**What a click can name is fixed and narrow: this tool of this app, by its exact name.** The
rule is `app.tool` (`github.issue_write`), taken by the daemon from the waiting change it holds,
never from the request, so a click cannot name anything else. A tool name a rule would read as
a pattern (`*`, `?`) is not offered. A whole app is not offered: "everything in github" on a
card about closing one issue is broader than the moment can make plain. Argument patterns are
not offered: `state=closed` in a few words on a card is easy to misread, and the rule forms are
still there in Permissions for whoever wants them. The words are the tool's name and the app's
key (`scope_words`), as ADR 0017 builds the words that ask for a call; never the app's
description, title or any other text it sends, and never the change's summary or rationale.

**The levels stay four, and the mode is untouched.** "Don't ask again" is `allow` (it runs, and
Ghost says so), "never" is `block`. The most careful match still wins, so the card never offers
a choice that would silently not take effect: when a rule of the operator's would still ask
(an `ask` rule naming the app, the tool, a glob over it, or a pattern this call matches), the
card says "Your rules ask for this every time: change that in Permissions." instead, and the
daemon refuses that allow if it is sent anyway. "Never" always takes effect.

**Who can set one: the operator, by a click in their own desktop session, and nothing else.**
That is the evidence a Confirm already is (`classifier: ui`, ADR 0013); the audit records
`permission_set` beside the decision, with the rule, the list and `by: ui`. The writer takes a
`Surface`, which has one value, `Desktop`.

- **Not the phone, yet.** A signed phone confirm is evidence for one yes (ADR 0051). A standing
  rule is a bigger thing than one yes: it outlives the moment, and the phone's card is small
  and often answered on the move. The phone keeps plain Yes and No. Adding it is one more
  `Surface` value and one route, a decision for review.
- **Not by voice, yet.** "Always" is a short word that is easy to mishear, and a model sits
  between the operator's words and any rule it could propose. A spoken yes stays one yes.
- **Never** a tool of Ghost's own app, an outside client, a run with nobody there, an app's text
  or a model. None has a path to the writer: no tool names it (`no_tool_makes_a_yes_or_a_no_stick`
  beside `no_tool_changes_a_permission`), only the control API's handlers call it
  (`the_writer_is_reached_only_from_the_control_apis_operator_routes`), and a run's change at
  `ask`, set aside on the waiting list (ADR 0047), can be made to stick only when the operator
  later decides it on the desktop
  (`a_runs_change_set_aside_is_made_to_stick_only_when_the_operator_decides_it`).

**One writer** (`ghost_core::permission_choices::{add_rule, remove_rule}`) edits
`policy.toml` as text through `toml_edit`:

- every byte outside the one array it touches stays as the operator left it: comments, order,
  spacing, other tables, a list written one item per line (the new item takes the spacing of the
  one before it);
- it refuses a file that does not read, and refuses an edit whose result reads as anything but
  the same permissions with that one rule more or less (an older layout where adding
  `[permissions]` would change the mode is refused, not rewritten);
- a rule already in the list is not added twice;
- it backs the file up once, before its first write (`policy.toml.before-choices`), and writes
  atomically: a temporary file beside it, synced, renamed over it with the original's mode;
- removing a rule that was the last thing added leaves the file byte for byte as it was,
  because the choice keeps whether the writer added the list's key or `[permissions]` itself.

What was set this way is kept in SQLite (`permission_choices`: the rule, the list, when, from
which change, from where) so Settings › Permissions can show it as a sentence ("“Issue write”
in github runs without asking · set 14 Nov from your yes · Remove") and remove it through the
same writer. The rest of Permissions (the mode, Try it, the rules in both forms and the file)
moves under "Advanced", unchanged in behaviour. Levels are shown to a person as *Never*, *Asks
first*, *Runs quietly* and *Runs without asking*; the code and the file keep `block`, `ask`,
`quiet` and `allow`.

## What it costs on a turn

Nothing. The gate reads its rules exactly as before, from the `AutonomyPolicy` behind an
`RwLock`; one more rule in a list is one more glob match per call. After a click the control API
calls `Controller::reload_policy`, which loads the file and swaps it into the running gate
(`WriteGate::set_autonomy`) before the change is confirmed, so the change itself and the very
next call follow the new rule with no restart
(`the_gate_follows_the_new_rule_on_the_very_next_call_without_a_restart`,
`yes_and_dont_ask_again_runs_the_change_and_the_next_one_without_asking`).

## Consequences

- `POST /proposals/{id}/always` (`{choice: "allow" | "block"}`), `GET /permissions/choices`
  and `DELETE /permissions/choices/{id}` on the control API only. A waiting change's view
  (`PendingProposal.always`) carries the offer: the rule, the words, and whether the allow would
  take effect.
- The surfaces: every card the desktop's own window shows has it (Today's Decide, the live
  strip on Today, the conversation screen), so one decision offers the same choice wherever the
  operator meets it. The tray, the floating room's fixed card (deliberately minimal, ADR 0059),
  a notification's buttons and the phone keep plain Yes and No, working exactly as before.
- Two writers already rewrote `policy.toml` from the operator's own screens (`PUT /permissions`,
  used by Permissions and an app's page, and `PUT /policy`, the TOML view and the routes). All
  three now take one lock (`ghost_core::policy_file`): a mutex in the process and an advisory
  `flock` on `policy.toml.lock` beside the file, so the daemon and a `ghostd` command never
  interleave either. Each re-reads the file under the lock before it edits, so two writes never
  lose a rule: both land, or the second is cleanly refused. A choice whose rule was taken out by
  hand is shown as no longer in the file.
