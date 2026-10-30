# 0041: Recall searches an agent's own transcript, kept longer, with FTS5 and no vector store

- **Status:** proposed (heka-vault/ghost#47)
- **Date:** 2026-10-29
- **Amends:** ADR 0014 (a transcript is searchable from the session it belongs to; the retention window is for summarized turns only)

An agent's memory is a rolling summary plus pinned notes (ADR 0014). What was said more than
a few sessions ago and never pinned survives only as the summary's paraphrase, and the turns
behind it are pruned after 90 days. The operator wants the words back: "what did we say
about the turn grace in August". A review of MemPalace (2026-10-29, `.scratch/mempalace-review.md`)
showed what a vector store costs here: 376 MB of dependencies, 280 MB resident, a shared
archive every agent can read, and verbatim replay of old untrusted text. Ghost needs none of
that for its own transcripts.

## Decisions

**Recall is a deterministic index over Ghost's own `turns` table.** An FTS5 external-content
table over `turns.text`, keyed on the turn id and kept in step by triggers, in the SQLite the
daemon already links (FTS5 is compiled into its bundled build). A database from before this
ADR rebuilds the index once when it first opens. Query words are sanitized by code before
they reach FTS5, since quotes, colons and hyphens are its syntax and the words arrive by
voice. Results are ordered by bm25 and carry a snippet. No embeddings, no model, no new
dependency.

**A `recall` read tool, scoped by construction to the agent in the session.** It sits beside
`list_agents` in the conversations source and takes a query and a limit. The session hands
it the active agent, as it already does for the source's changes; the model never names
whose transcript to search, so ADR 0014's private memory holds. Hidden turns (a reset or a
delete) are never returned. The result is shaped for a spoken turn: a handful of hits, each
with when, who, and a short snippet, since an in-tree read result reaches the model as it
is. It is a read: the operator's permissions can block `conversations.recall`, and it is not
in the audit. Passwords never enter the index because the store redacts a turn before it is
written. What comes back is the operator's and Ghost's own earlier words, and the one-line
call records; tool results were never stored and so are never recalled.

**Retention is split.** `GHOST_TURN_RETENTION_DAYS` keeps its meaning for summarized turns,
and `0` keeps them for good; the default moves from 90 days to keeping them. Hidden turns are
pruned after a short fixed window instead, so a reset does not leave its transcript
searchable for years. Pruning runs daily, as the setting's comment already claims; today it
runs only when the daemon starts. Keeping turns changes what the prompt loads not at all:
resume still loads the summary, the notes and the turns no summary covers yet.

**Built once for meetings too.** ADR 0040 keeps meeting segments in SQLite with a read tool
of their own. The FTS5 helper is written over a table and a text column so that tool reuses
it rather than growing a second index.

## Considered options

- **A vector memory app (MemPalace or similar), connected as an app.** Rejected for Ghost's
  own transcripts: it cannot be private per agent (ADR 0035 lets nothing narrow an app per
  agent), it replays tool output by design, and it costs hundreds of megabytes for a store
  that holds kilobytes. It stays a possible optional app for an archive filled from the
  operator's other tools, which is a different decision.
- **Keep turns longer, search nothing.** The desktop can scroll a transcript, but by voice a
  longer retention without a search tool answers nothing.
- **`LIKE` over `turns.text`.** Enough for today's few hundred turns, but no ranking, no
  snippet, and nothing for meetings. FTS5 costs the same lines and ranks.
- **A semantic index in-process (an ONNX embedder plus a scan).** Not now: keyword recall
  over the operator's own words is deterministic and testable; a model-made index is a step
  the turn pays for, and ADR 0018 wants its cost measured before it is added.

## Consequences

- One small tool's schema joins every turn's prompt; `make perf` runs before and after, and
  the figures go in the PR.
- Disk grows by the transcript itself, about a megabyte a year at today's use.
- `docs/architecture.md`'s memory section and ADR 0014's retention line are updated when
  this is built. The control API's turns endpoint may take a `q` parameter and the desktop
  Transcript screen a search field; neither is needed for the voice path.
