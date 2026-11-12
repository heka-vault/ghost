# 0046: Apps keep up with what they offer

- **Status:** accepted (decided 2026-11-11; built on the `feat/apps-parity` branch, heka-vault/ghost#65: the live tool list, calls that can be stopped, reconnection, the result budget, descriptions and instructions, resources and prompts as reads, safe names and deferred tools; what is not built is listed below)
- **Date:** 2026-11-11
- **Amends:** ADR 0034 (what the model reads of an app: a description up to a cap and not
  its first sentence, the app's own instructions, a result held to one budget and not cut
  string by string), ADR 0045 (Ghost's own app is no longer the only one whose
  descriptions reach the model past the first sentence; one tool joins it), ADR 0018
  (what each of these costs on a turn, measured below), ADR 0021 and the idle timeout
  (a call in flight is not the operator gone quiet)

Ghost connected an app the way a first client does. It listed the tools once and never
again, gave a call 20 seconds, read 1,200 characters of each string that came back,
showed the model the first sentence of each description, left a dead connection dead,
and ignored everything an app offers besides tools. An agent could not read a whole
issue, could not wait for a slow report, and lost an app for the rest of the day when its
process restarted. The leading open-source agents (Goose, Hermes Agent) do all of this,
and the owner asked for the same level.

## Decision

The client keeps up with the app. What stays as it was: every tool that is not read-only
is a change through the gate, with the audit and the operator's `block` and `ask` rules;
every password Ghost holds is replaced in everything an app sends before anything reads
it (ADR 0036); and the prompt tells the model that what an app sends is data.

**The tool list is live.** Ghost's side of an app's session is a real handler
(`ghost_core::apps::connection::Client`), not the empty one. When the app says its tools
changed (`notifications/tools/list_changed`) Ghost lists them again. A counter is bumped
on the notice, and a listing is kept only if the counter did not move while it ran, so a
slow listing never overwrites a newer one (Goose's version guard). The existing
`Apps::on_change` hook fires, so the gate's registry follows. An open session asks for
its tool list before every model call and replaces the list, and the apps' paragraphs of
the prompt, only when the list differs: an app connected, disconnected, reconnected or
changed in the middle of a session is offered from the next model call, and the cached
prompt is lost once per change, not once per turn.

**A call may take minutes, and can be stopped.** A call may take 300 seconds and
connecting 60 (both leaders' defaults). A call stops when the turn it was made for is
cancelled (the operator spoke), when the operator hangs up, when the session ends, or
when its time is up, and the app is sent `notifications/cancelled` for the request. The
call returns an error the model reads, so the context still has a result for every call
and a change through the gate still has its outcome in the audit.

What was found when a call runs for a minute: the model's stage waited on its calls
without watching the turn. The microphone, speech recognition and the turn detector are
upstream and kept working, so the operator's words did start a new turn, but everything
that turn sent down the pipeline queued behind the call: the thinking sound kept playing
over the operator, the new turn was not answered, "hang on, Ghost" and "goodbye" were
taken by their interceptors but the hold's and the hang-up's frames waited too, and the
desktop's End did nothing until the app answered. With 20 seconds that was a bounded
annoyance; with 300 it would have been a session the operator could not get out of. The
idle timeout (45 s) also fired during a long call, and hung up as soon as it returned.
Now the stage stops its calls on the turn's token and on a hang-up
(`TurnControl::stop_calls`), and the idle timeout does not count while a call is in
flight.

**A dead connection is opened again.** An app whose connection died keeps its tools in
the list. The next call to it opens it again, once, before failing. A read that was cut
off by the connection dropping is made again after the reconnect; a change is not, since
the app may have made it, and the model is told to check. In the background
(`Apps::supervise`) every connection is looked at every 5 seconds; one that died, or
never started, is tried again with the wait doubling from 1 second up to 60 for five
tries, each off by up to 20 percent either way, and then every 300 seconds (Hermes
Agent's numbers). An app over HTTP that has been quiet for 180 seconds is pinged, unless
it says it does not answer pings. An app that needs a new sign-in is not retried: it
waits on the operator's list as before. An app the operator disconnected is never opened
again, whoever still holds it.

**A result has one budget.** One result is read whole when it is within 24,000
characters after redaction. Past that it is cut at the end and carries a note saying how
much was dropped and to ask for less. A list keeps 200 items within the budget.
Structured content comes back as the JSON the app gave. An image, a sound or a file is
named with its type and size instead of being dropped silently; a text file the app
embedded is read as text. An app's own error message is passed on, redacted and cut to
300 characters, where it was reduced to a kind.

**The model reads what the app says about its tools.** A tool's description reaches the
model up to 1,024 characters, with the schema's own property descriptions, both
redacted. What the app says about itself when it connects (its `instructions`) joins its
paragraph of the prompt, up to 2,000 characters (OpenClaw's cap), inside the sentence
that calls an app's words data. The operator's list of an app's tools keeps the first
sentence.

**Documents and prompts.** When an app advertises resources or prompts, Ghost offers
reads over them (Hermes Agent's pattern): `<app>_list_resources`,
`<app>_read_resource(uri)`, `<app>_list_prompts`, `<app>_get_prompt(name, arguments)`.
Only when advertised, within the same result budget, and under the app's name in the
permissions like any read. Ghost keeps no copy of either list, so a changed list is
never stale.

**A question for the operator.** An app may ask the operator something in the middle of
a call (the protocol's elicitation, form mode). The question is redacted, cut to 500
characters and handed to an `Asker` (`ghost_core::apps::Asker`: the question, the
choices when the answer is one field with a fixed list, the schema, a time limit) and the
answer, a decline or a cancel goes back to the app. The asker that exists today declines
everything (`DeclineAll`), and until a real one is set (`Apps::set_asker`) Ghost does not
tell apps it takes questions. A question that sends the operator to a web address (URL
mode) is declined before it reaches an asker. A result that says "input required" (the
newer protocol's way of asking) fails the call with what was asked; see below for why it
cannot arrive today.

**Names.** A name offered to the model matches `[a-zA-Z0-9_-]{1,64}`. A name the model
cannot take is rewritten, and one that is too long or already taken gets a short hash of
its whole name on the end (Goose's `provider_safe_tool_name`), so `db.query` and
`db_query` are two tools. A clash used to leave the later tool out with a problem on the
app's page. Which app a tool belongs to travels with the tool (its `ToolSource`), never
read back out of the name.

**Many tools.** Past 40 tools or 30,000 bytes of schema across the connected apps, only
the tools the operator pinned (`pinned` in `apps.json`) and the twelve the model used or
found most recently are offered by name. The rest are found with `ghost_tools_search`, a
read of Ghost's own app (`ghost_core::ghost_app::tools_search`): a few words give up to
eight tools, each with the name to call it by, what it does and its arguments, and those
are offered by name from the next model call. A hidden tool is still registered with the
gate and can be called by name. Within the limits the search tool is not offered and
nothing is hidden.

### Each limit, before and after

| Limit | Before | After | Set with |
|---|---|---|---|
| One call | 20 s | 300 s | `timeout` per app, `GHOST_APP_TIMEOUT` |
| Connecting | 15 s | 60 s | `connect_timeout` per app, `GHOST_APP_CONNECT_TIMEOUT` |
| A result | 1,200 characters per string, 30 items per list, no bound on the whole | 24,000 characters for the whole, 200 items per list, a note when cut | `result_chars` per app, `GHOST_APP_RESULT_CHARS` |
| What a change answered (`state_now`) | 1,200 characters | 24,000 characters | the same |
| A string argument | 16,000 characters | 200,000 characters | |
| A list argument | 100 items | 1,000 items | |
| A tool's description to the model | first sentence, 240 characters | 1,024 characters | |
| The app's instructions | not read | 2,000 characters in its paragraph | |
| An app's error message | its kind only | its words, redacted, 300 characters | |
| The tool list | listed once at connect; a session's list replaced only on a transfer | listed again when the app says so; a session's list replaced before a model call when it differs | |
| A dead connection | stayed dead until something else revived it | opened by the next call, and in the background | |
| A name two tools would share | the later one not offered | the later one offered with a hash on its name | |
| Tools offered by name | all of them | all up to 40 tools and 30,000 bytes; past that the pinned and the 12 most recent | `GHOST_APP_TOOLS_MAX`, `GHOST_APP_TOOLS_BYTES`, `GHOST_APP_TOOLS_KEPT`, `pinned` per app |

## Consequences

Cost on a turn (ADR 0018), measured:

- **The default seed** (mock GitHub and Ghost's own app,
  `ghost_app::tests::the_default_seeds_tool_list_is_measured`): the advertised list is
  14 tools and 5,367 bytes of schema before and after. Mock GitHub's descriptions are one
  sentence each, so the longer cap adds nothing there, and the search tool is not offered
  under the limits. The apps' paragraphs of the prompt go from 1,015 to 1,033 bytes (the
  sentence that calls an app's words data now covers what it says about itself).
- **A wordy app.** A tool whose description is 1,024 characters costs up to 784
  characters more than it did, about 200 tokens, on every turn, in the cached prefix. An
  app's instructions cost up to 2,000 characters once per app. A hundred such tools would
  be 100,000 characters, which is why the limits above exist: past 40 tools or 30,000
  bytes only the pinned and the recent are sent.
- **Asking for the list before each model call.** The list is built again and compared:
  0.4 ms with the seed's 17 tools, 1.0 ms with 39 app tools, 6 ms with 200 app tools
  (deferred), in an unoptimised test build on the workstation. Not measured in a release
  build. Before, it was built once per session.
- **A result.** One result can now put 24,000 characters, about 6,000 tokens, in the
  context, where a single string was 1,200. The old rule did not bound the whole: thirty
  issues with bodies came to 36,000 characters and more. The new one does.
- **Finding a tool** costs one extra model call, the search, the first time a hidden tool
  is needed, and one loss of the cached prompt when it joins the list.

Other consequences:

- The model reads more of what an app wrote: its descriptions, its instructions, its
  errors. That is the loosening the owner asked for, and it widens what a hostile app can
  put in front of the model. The defences are the ones that were there: the prompt calls
  it data, the gate asks or blocks by the operator's rules whatever the model was told,
  and no app text sets a permission. The Mobbin case (a description that had Ghost read
  a billing notice aloud) can happen again with a long description; the framing sentence
  is what stands against it.
- A cancelled change may or may not have been made by the app. The model is told so and
  told to check before trying again; the audit row says the call was stopped.
- A tool that was offered and is now hidden can still be called by name. Hiding is for
  the size of the prompt, never a permission: blocking is the operator's file.
- The tools the model used are remembered in memory, for the daemon's run. After a
  restart the pinned ones are offered and the rest are found again.
- The background look tries the apps one after another; an app that takes its whole
  60 seconds to fail delays the look at the others by that long.

## What is not built

- **The operator's side of a question.** `Asker` has only the declining implementation.
  The real one (say it, show it, wait for the answer) plugs in with `Apps::set_asker`
  before the apps connect.
- **The newer handshake.** Ghost opens every app with the `initialize` handshake, which
  every app speaks. Under it a question arrives as a request, which is handled. The
  2026-07-28 protocol asks with an "input required" result instead, and `rmcp` refuses to
  send one to a client on the older handshake, so that path is an error message and a
  unit test, not something seen live. Moving to `rmcp`'s discover lifecycle is its own
  decision: it probes for ten seconds against an app that does not know it.
- **Tasks.** A tool that answers "I started a task" is reported as such, not followed.
- **Progress.** A call's time limit does not reset when the app reports progress.
- **The older HTTP+SSE transport.** `rmcp` 3.9 has stdio and Streamable HTTP clients and
  no client for the transport that came before; an app that speaks only that cannot be
  connected.
- **Sampling.** An app cannot ask Ghost's model to write for it. The protocol has
  deprecated it.
- **Images and sound.** They are named with their size, not shown to the model.
- **The desktop.** `timeout`, `connect_timeout`, `result_chars` and `pinned` are keys in
  `apps.json`; there is no control for them on the app's page, and no API type changed.
- **Which tools were used, across restarts.** Could be read from the audit at start.

## Alternatives considered

- **Keep the first sentence, raise only the result.** The first sentence was chosen to
  starve prompt injection. It also starved the model: a tool whose second sentence says
  "returns at most 30; use `page`" was called badly. Both leaders send the whole
  description.
- **Drop the future to cancel.** Simply abandoning a call tells the app, since `rmcp`
  sends the notice when the request is dropped, but dropping a change in the middle of
  the gate leaves a proposal with no outcome. The call observes a token and returns an
  error through the gate instead.
- **Try a change again after a reconnect.** The app may have made it before the
  connection dropped, and a second comment or a second payment is worse than an error.
- **Hide tools per agent.** ADR 0035: there is one set of permissions and nothing narrows
  an agent. Deferral is the same for every agent and is not a permission.
