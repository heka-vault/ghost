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
