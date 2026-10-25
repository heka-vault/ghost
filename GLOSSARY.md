# Glossary

The words Ghost uses, one meaning each. The operator hires an **agent** into a
**department** and talks to it; what is said is the **conversation**. The phone metaphor
of the first increment (*line*, *patch me through*) is gone (ADR 0024).

| Term | Meaning | Not |
|---|---|---|
| **Agent** | What the operator hires, talks to, moves and retires: its own name, job, persona, voice, private memory, notes and transcript, working in one department. Ghost is agent zero. The desktop's Agents screen and the phone's Home list them. The code and the control API call the same object a *conversation* (`ghost_core.conversations`, `/conversations`); the word to show is **agent**. | A session; a department. |
| **Department** | A folder under `~/.config/ghost/departments/` (Ghost's is the config folder itself): its rules (`AGENTS.md`), servers (`mcp.json`), what the gate may do with them (`department.json`), skills, docs and a shared memory every agent in it reads (ADR 0024). Each has a default agent named after it; "transfer me to accounting" lands there. A session in a department reaches only what its folder gives. | A permission bundle on its own (that was a *toolkit*, retired). |
| **Conversation** | What is said with an agent: its transcript, with the agent's memory of it. Opening an agent in the desktop shows the conversation first; the agent's settings sit behind it. In code, also the storage name for the agent. | |
| **Session** | One stretch with the mic open, from a wake or a call to a hang-up. A session starts in one agent (Ghost at the desk; on the phone, Ghost or the agent you called) and can be transferred to another. | |
| **Call** | A phone session, started from the phone's Home: the ghost for Ghost, a key for a department or an agent. | |
| **Transfer / switch** | Moving a running session to another agent; "back to Ghost" returns. Retired agents are out of reach by voice. | A *move* (below). |
| **Move** | Putting an agent in another department: its folder, memory and transcript go with it; a running session in it follows (ADR 0024). A department's default agent is not moved; a new one is hired in the destination with a summary. | A transfer, which moves the session. |
| **Hire / retire / rehire** | Create an agent in a department; take it out of voice routing and the active list, transcript and audit kept; bring it back. Delete removes it for good. | |
| **Rule** | A line of a department's `AGENTS.md`, in every one of its agents' prompts. "Add a rule" appends the operator's words verbatim through the gate. | A persona, which is one agent's. |
| **Owner** | The department whose server allowlist names an item (a repository). A request about it from elsewhere gets the owner's name and an offer to transfer. | |
| **Checked out (hold)** | "Hang on, Ghost": the session stays, Ghost stops listening until addressed by name at the start of an utterance; nothing heard meanwhile is kept (ADR 0021). | Hung up. |
| **Hang up** | A goodbye ends the session; Ghost says a short goodbye and the desk returns to passive (ADR 0021). The End button on the phone ends it silently. | "Go to sleep" (gone). |
| **Passive** | The desk listening locally for the wake phrase only; nothing leaves the machine (ADR 0010). | |
| **Gate** | The one path for every write: policy level, snapshot, bounds, read-back, audit (ADR 0007, 0011). | |
| **Skill** | A `SKILL.md` folder: instructions the model reads on demand through `read_skill`, never a source of permission. Read nearest first: the department's, the operator's `~/.agents/skills`, Ghost's own. Scripts in it run only through the gated scripts server (ADR 0023). | |
| **Server** | An MCP server a department declares in its `mcp.json`, with its reads, scope, writes and levels in `department.json`: its reads are offered to the model, its writes by declaration through the gate (ADR 0022, ADR 0024). A department may `use` one of Ghost's. Also *source*: a server, the agents or the departments, as the gate and the audit know them (the audit's `ability` field holds the source's name). | An *ability*: the installable packages Ghost had until 2026-10-20 (ADR 0012, superseded). |
| **Journal** | A department's writes in the audit, by any of its agents, newest first. | |
| **Wisp** | Ghost's mascot and the only ghost drawing in the apps; an agent is shown as Wisp in the agent's colour. | |

Retired words, so old notes still read: a **line** was an agent; a **toolkit** was a
permission bundle in the policy file and a **template** a folder that stamped one out,
both replaced by the department folder; a **settled** line is a retired agent; a thread's
**scope** is now its department's allowlists.
