# 0019: Threads scope work; Ghost stays unscoped

- **Status:** accepted; the per-thread scope was superseded by ADR 0024 and the allowlists and ownership routing by ADR 0034
- **Date:** 2026-10-19
- **Amends:** ADR 0014 (a conversation gains a scope and a settled state; "same abilities and policy in every conversation" ends; a thread Ghost creates by voice starts with a persona Ghost wrote), ADR 0012 (abilities declare how they can be scoped)

The operator will use Ghost against several GitHub organisations, the workstation, Notion
and more, and wants threads (ADR 0014's conversations) as the way to keep that work apart:
a thread per project, or several per project ("heka new" and "heka workshop"), each with
its own persona, voice and memory, so the voice alone says which project is in play and
nothing crosses streams by accident. GitHub is the proof-of-concept ability; the design
below is generic and GitHub is only the first ability to fill it in.

## Decisions

**A thread has a scope: per ability, a list of what it may touch.** The profile gains
`scope: {ability: [item, ...]}`, for example `{"github_issues": ["heka-vault/ghost"]}`.
An ability declares how it can be scoped (`Contributions.scope_kind`: "repository",
"directory", "workspace") and what the whole allowlist is; a thread's scope is a subset.
Several threads may share a scope: scope is about what the thread sees, memory is what
makes threads different.

**The gate enforces scope.** With a scoped thread active, a write whose target is outside
the thread's scope is refused before policy, autonomy or the model's wording come into
it. Read tools are narrowed the same way (the ability sees the active scope and lists
only what is in it), and the ability's prompt paragraph names only the scoped items.
Where a thread has exactly one item, tools stop requiring it ("close seventeen" needs no
repository).

**Ghost (line zero) is unscoped.** It sees every allowed item of every ability, as today.

**Crossing streams is the operator's setting, not Ghost's judgement.** Two settings in the
desktop app, both off by default:

- *Cross-department answers.* On: a scoped thread may read outside its scope to answer a
  question ("is app 151 still open?") but never write there. Off: it says that belongs
  to another thread.
- *Department routing.* On: when a request belongs to another thread, Ghost patches the
  operator through to it (announced, as switching is today) and that thread handles it.
  Off: Ghost says which thread it belongs to and offers to switch.

Writes outside scope are refused in every configuration. The two settings together give
the three behaviours the operator wants to compare: refuse, answer read-only, or route.

**Threads are created by voice or in the desktop app.** By voice ("make this a thread for
the ghost repo", "save this as heka workshop") Ghost creates it from the recent
conversation: it picks the name, binds the scope the conversation was about, picks an
unused voice, and writes a short persona of its own for the thread, drawn from what the
conversation was about, so a new thread never starts as a blank template and each one
sounds different. This amends ADR 0014, which stored persona text only verbatim from the
operator: that rule still holds for *changing* a persona by voice (`set_persona`), where
the operator's exact words are the point. A persona Ghost wrote at creation is a draft the
operator is expected to edit in the desktop app. Creation stays a gate operation at its policy level.

**When a thread's context has drifted, settle it and start another.** `settle` marks a
thread done: it leaves voice routing and the active list, keeps its transcript, memory
and audit readable in a "Settled" group in the desktop app, and can be unsettled. A new
thread for the same scope may be seeded from the settled one's `memory.md` as a handoff
the operator can edit first. `reset` (wipe memory and transcript in place, confirm-level)
stays for the operator who wants the same thread clean. Settle is announce-level; it
destroys nothing. Ghost does not decide a thread has drifted; that stays a judgement the
operator makes, and automatic signals are a later, advanced feature.

**One ability, several credentials.** An ability may need a credential per scope owner
(a token per GitHub organisation). The ability declares a credential map in its settings
(`GHOST_GITHUB_TOKENS="heka-vault=HEKA_ORG_TOKEN,quotient=QUOTIENT_ORG_TOKEN"`, each value
the name of an environment variable holding the secret) and picks by owner, falling back
to `GITHUB_TOKEN`. Secrets still come only from the environment and are never written to a
file or a log.

## Consequences

- `ghost_core` gains the scope model, the gate check, a per-session "active scope" the
  pipeline hands to abilities at a switch, and `settle`/`unsettle` operations under
  `[conversations]`. Abilities that declare no scope kind are unaffected.
- `github_issues` declares repositories as its scope kind, narrows its prompt, read tools
  and `list_awaiting_signoff` to the active scope, makes `repo` optional in a one-repo
  thread, and resolves the token by owner.
- The desktop app's line page gets a Scope section and a Settle action; the lines index
  gets a Settled group; Settings gets the two crossing-streams toggles and the token map.
- The phone shows the thread's name as today; scope needs no phone change.
- The policy file is unchanged. Per-target levels (`[github_issues.targets."o/r"]`) still
  apply inside a scope.
- Ghost's own memory stays private to Ghost; a handoff into a new thread is one edited
  summary, as transfers are today (ADR 0014).
