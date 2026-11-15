# 0061: Agents live on the rail, each with its own look

- **Status:** accepted
- **Date:** 2026-11-12
- **Amends:** ADR 0035 (an agent's fields), the overhaul's frame (docs/design/overhaul-2026-11.md: "no agent list" on the rail)

The owner, using the desk live: the Agents item opened a settings page and there was no list of agents to call or chat with; agents were told apart by colour only; and Apps, "too important a feature to get hidden in settings", lived under Settings.

## Decision

**An agent has a look: a shape and a colour**, kept as one field on the agent (`look`, `"<shape>:<colour>"` in its `agent.json` and the registry's mirror, a `look` column added to `conversations`). The shapes are a family around Wisp (`wisp`, `drop`, `bell`, `hood`, `cloud`, `lantern`, `star`, `moth`), each a small ghost with Wisp's two eyes, told apart by silhouette first so two agents in one colour, or a colour-blind operator, still tell them apart at 20 px; the colours are the eight agent hues. Ghost is always Wisp in colour 0; an agent with no look is Wisp in the colour its slug picks, as before. It is set when hiring and changed in the agent's settings, through the gate like a rename (`Create.look`, `Rename.look`, `ghost_core::conversations::models::parse_look`); a look the file got wrong is dropped, never refused, so the agent still loads. No layer is added: a field on the agent, as ADR 0035 asks.

**The rail lists the agents themselves.** Under Today, Watching and Apps: each agent's mark and name, its live state on the mark (a ring in the voice phase's colour while it is in the session, a pulse while it runs, a count while it waits on the operator), and + to hire one. Choosing an agent opens the conversation with it (`#/agents/:slug`): typing there while a session is with another agent transfers the session first. The roster and each agent's settings (persona, voice, look, memory, history, retire) are under Settings › Agents (`#/settings/agents`, `#/settings/agents/:slug`). Ghost stays the presence at the top, never a row; retired agents are off the rail.

**It scales by order and by scroll, not by grouping.** The agent in the session first, then those waiting on the operator, then those at work, then the rest by when they last worked (`app/railAgents.ts`). The list scrolls inside the rail's nav with a fade at its foot, so the rail's foot never moves; + stays at the head of the list; under 760 px of height the names go and the marks stay, each with its tooltip and accessible name.

**Apps is its own place** (`#/apps`, `#/apps/:name`), not a page of Settings; `#/settings/apps…` redirects.

## Amendment, 2026-11-14

The `hood` shape is retired: the operator did not want it in the family. A look that still
names it is read as no look, so that agent is drawn as Wisp in the colour its slug picks.

## Amendment, 2026-11-14: fewer places, and the agents capped

The owner asked for a simpler desk after a review of the field ("I want to make sure we are not
having an overly complex UI"). The measure was a newcomer's: places, clicks, and nouns to learn.

**Four places and the agents.** The rail is Ghost's presence, Today, **On its own**, Apps, the
agents, then Settings, with the floating switch and the microphone at the foot. *Watching*
became *On its own* (`#/on-its-own`; `#/watching` and `#/departments/…` redirect): one list of
everything Ghost does without being asked, each row a sentence, made by asking Ghost in words
(a typed turn: the agent uses `ghost_schedule_manage` and `ghost_watch_manage` through the gate
as before), with the forms one disclosure down (Set it up by hand). The Hearing test left the
rail (ADR 0042 is unchanged): Settings › Sound's "Test what Ghost hears", the microphone's note
there (ADR 0064), Wake takes and the first run lead to it.

**The agents are capped.** The rail shows every agent that matters now (in the session,
waiting on the operator, at work), then the most recently used, as many as fit without
scrolling and six at the most (`railShown`, `RAIL_MAX`; measured: four at the window's default
1040×720, five at 1440×900). The rest, and hiring (+ moved off the rail into the roster), are
one entry, **All agents**, which opens Settings › Agents. Nothing is pinned: with the agents
that matter always shown, the cap was usable without it.

**Order by hire, not by state.** This replaces "the agent in the session first, then those
waiting…": the agents shown are listed in the order they were hired, so an agent keeps its
place and its state shows on its mark. While the pointer or focus is on the list, a newcomer
goes at the end and nothing leaves or moves until it goes (`holdStill`), so a row never jumps
under the pointer. At the smallest window (480px tall) the places and the foot take the whole
rail, so the agents that matter are there but the nav scrolls, as it did before the cap.
