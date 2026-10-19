# 0014: Named conversations persist across sessions, each with its own persona, voice and memory

- **Status:** accepted; amended by ADR 0015 (no undo), ADR 0019 (scope, settle, and a persona Ghost drafts for a thread it creates by voice)
- **Date:** 2026-10-18
- **Amends:** the "fresh context per session" assumption in [architecture.md](../architecture.md)

Every wake used to start from an empty LLM context. The operator wants to "save a
conversation as accounting", be "patched through to accounting" after a later wake, and
carry on where that thread left off, in that thread's voice and manner. Ghost itself
keeps a primary thread that can be reset.

A **conversation** is a profile (name, spoken aliases, persona, Kokoro voice, speed), a
transcript, and a memory. `ghost` is conversation zero: always present, undeletable,
resettable, the same shape as every other. A wake always lands in `ghost`; switching is
an in-session LLM tool that swaps the context, the system prompt and the TTS voice in the
running pipeline. Sleep works from anywhere. Every spoken turn persists as it happens.

**Storage** follows the prior art: the tools that became multi-conversation (Goose,
opencode, OpenClaw, Cursor) all moved transcripts from JSONL to SQLite, and the ones with
curated memory (Claude Code, OpenClaw) keep it as Markdown the operator can open and fix.
Transcript rows, the conversation registry and every compaction version live in Ghost's
SQLite. `persona.md`, `memory.md` and `notes.md` live under
`~/.local/share/ghost/conversations/<slug>/`, written by Ghost only through the gate's
local-file target, so they are snapshotted, audited and undoable like any other write.

**Memory** is a rolling summary in `memory.md` plus verbatim pinned notes in `notes.md`
that are never summarized. Compaction is Pipecat's in-session summarizer with gentle
settings (tokens only, 24k trigger, last 20 messages kept, summary up to 3k, all
tunable). Every applied summary is recorded as a version; the file holds the current one
and is the source of truth on resume, so an edit by hand is folded into the next version.
Summarized turns stay in SQLite, flagged, and are pruned by a retention setting (90 days).
Raw tool results are not stored: only spoken turns and one-line call records re-enter a
later session, so old untrusted text is never replayed.

**Privacy of memory.** A conversation loads only its own persona, memory and notes.
Ghost's primary memory never enters a named conversation and vice versa. A transfer
carries one handoff line.

**Autonomy.** Conversation mutations are gate operations under `[conversations]`:
create, rename, set_voice, remember and compact announce; set_persona, forget, reset
and delete confirm. Switching is a session action, not a write; the policy may deny it.
Persona text shapes the prompt, so it is typed in the desktop app or stored verbatim
from the operator's own words at confirm; the voice and speed may be set by speech
because they are an enum.

## Consequences

- Session transcripts are now stored on the machine. The passive rule is unchanged:
  nothing before the wake phrase is transcribed into storage.
- Each conversation has its own identity from its persona, with Ghost's base rules
  (spoken style, untrusted data, the gate) underneath.
- Same abilities and policy in every conversation for now. The policy file may later
  grow a per-conversation scope.
- Wake is always "hey ghost"; per-conversation wake phrases are not offered.
- A switch cancels any proposal waiting for a yes. Undo stays session-scoped.
