# 0041: Recall searches an agent's own transcript, with FTS5 and no vector store

- **Status:** accepted (proposed 2026-10-29 in heka-vault/ghost#48 for #47; built 2026-11-12 with ADR 0052 on the `feat/own-tools-2` branch: the index, `ghost_recall` and `ghost_transcript_read`. What was proposed and is not built is listed under "Where this departs")
- **Date:** 2026-10-29, accepted 2026-11-12
- **Amends:** ADR 0014 (a transcript is searchable by the agent it belongs to, and by Ghost)

An agent's memory is a rolling summary plus pinned notes (ADR 0014). What was said more than
a few sessions ago and never pinned survives only as the summary's paraphrase, and the turns
behind it are pruned after 90 days. The operator wants the words back: "what did we say
about the turn grace in August". A review of MemPalace (2026-10-29) showed what a vector
store costs here: 376 MB of dependencies, 280 MB resident, a shared archive every agent can
read, and verbatim replay of old untrusted text. Ghost needs none of that for its own
transcripts.

## Decision

**Recall is a deterministic index over Ghost's own `turns` table** (`ghost_core::recall`).
An FTS5 external-content table over `turns.text`, keyed on the turn id and kept in step by
three triggers (insert, delete, and an update of the text), in the SQLite the daemon already
links (FTS5 is compiled into its bundled build). A database from before this builds the
index once when it opens. Query words are cut into plain words by code before they reach
FTS5, since quotes, colons, hyphens and `*` are its syntax and the words arrive by voice;
each is matched as a quoted term, a prefix counting. Every word must match, and when nothing
does, any word may. Results are ordered by bm25 and carry a snippet with what matched marked.
No embeddings, no model, no new dependency.

**`ghost_recall(query, agent?, since?, limit?)` and `ghost_transcript_read(agent?, before?,
limit?)`, reads of Ghost's own app** (ADR 0045). The calling agent comes from the session's
dispatcher (`_agent`), never from an argument the model writes. An agent searches its own
transcript; Ghost (agent zero) may name any agent, or all. A hit says which agent, who said
it (the operator, the agent, a tool call, a note), when, what kind of turn it is (a session,
a scheduled run, a turn an outside tool or another agent asked for), and the turn either
side in the same session. Hidden turns (a reset or a delete) are never returned. Results are
held to an app's result budget. The operator's permissions can block `ghost.recall` like
any read. Passwords never enter the index because the store redacts a turn before it is
written; tool results were never stored and so are never recalled; passive audio and
passive transcripts are never stored and so can never be found.

**An outside tool** (ADR 0048) is offered both reads over the turns of the runs it started
and nothing else: its `agent_ask` turns, whose session is `run-ask-<client>-<id>`, and the
runs of the schedules it set up.

## Where this departs from the proposal

- **Not in the conversations source.** It was to sit beside `list_agents`. ADR 0045 made
  Ghost's own app the place for an agent's view of Ghost, so it is a tool there, and an
  outside tool can be offered it with its own scope.
- **Ghost reads any agent's.** The proposal scoped recall by construction to the agent in
  the session. Agent zero manages the other agents (schedules, questions), and the owner
  asked that it reach everything Ghost can; an agent other than Ghost still reads only its
  own, so ADR 0014's private memory holds between agents.
- **Retention is unchanged.** Keeping summarized turns for good by default, pruning hidden
  turns after a short fixed window, and pruning daily are not built: `GHOST_TURN_RETENTION_DAYS`
  is still 90 and pruning still runs when the daemon starts. A pruned turn leaves the index
  in the same statement.
- **No meeting reuse yet.** The helper is over the `turns` table only; ADR 0040's meeting
  segments do not exist yet.

## Considered options

- **A vector memory app (MemPalace or similar), connected as an app.** Rejected for Ghost's
  own transcripts: it cannot be private per agent (ADR 0035 lets nothing narrow an app per
  agent), it replays tool output by design, and it costs hundreds of megabytes for a store
  that holds kilobytes.
- **Keep turns longer, search nothing.** By voice a longer retention without a search tool
  answers nothing.
- **`LIKE` over `turns.text`.** No ranking and no snippet, for the same lines of code.
- **A semantic index in process.** Keyword recall over the operator's own words is
  deterministic and testable; a model-made index is a step the turn pays for, and ADR 0018
  wants its cost measured first.

## Consequences

- `ghost_recall` is one of the tools every turn carries (ADR 0052's core): 856 bytes of
  schema. `ghost_transcript_read` is found by search.
- Disk grows by the index, about the size of the transcript's text again.
