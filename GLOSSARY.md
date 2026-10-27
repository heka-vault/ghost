# Glossary

The words Ghost uses, one meaning each. The operator hires an **agent** into a
**department** and talks to it; what is said is the **conversation**. The phone metaphor
of the first increment (*line*, *patch me through*) is gone (ADR 0024).

| Term | Meaning | Not |
|---|---|---|
| **Agent** | What the operator hires, talks to, moves and retires: its own name, job, persona, voice, private memory, notes and transcript, working in one department. Ghost is agent zero. The desktop's Agents screen and the phone's Home list them. The code and the control API call the same object a *conversation* (`ghost_core.conversations`, `/conversations`); the word to show is **agent**. | A session; a department. |
| **Department** | A folder under `~/.config/ghost/departments/` (Ghost's is the config folder itself): its rules (`AGENTS.md`), its title, voice and permissions (`department.json`), skills, docs and a shared memory every agent in it reads (ADR 0024). Each has a default agent named after it; "transfer me to accounting" lands there. It reaches every connected app, less what its permissions block (ADR 0034). | A permission bundle on its own (that was a *toolkit*, retired). |
| **Conversation** | What is said with an agent: its transcript, with the agent's memory of it. Opening an agent in the desktop shows the conversation first; the agent's settings sit behind it. In code, also the storage name for the agent. | |
| **Session** | One stretch with the mic open, from a wake or a call to a hang-up. A session starts in one agent (Ghost at the desk; on the phone, Ghost or the agent you called) and can be transferred to another. | |
| **Call** | A phone session, started from the phone's Home: the ghost for Ghost, a key for a department or an agent. | |
| **Transfer / switch** | Moving a running session to another agent; "back to Ghost" returns. Retired agents are out of reach by voice. | A *move* (below). |
| **Move** | Putting an agent in another department: its folder, memory and transcript go with it; a running session in it follows (ADR 0024). A department's default agent is not moved; a new one is hired in the destination with a summary. | A transfer, which moves the session. |
| **Hire / retire / rehire** | Create an agent in a department; take it out of voice routing and the active list, transcript and audit kept; bring it back. Delete removes it for good. | |
| **Rule** | A line of a department's `AGENTS.md`, in every one of its agents' prompts. "Add a rule" appends the operator's words verbatim through the gate. | A persona, which is one agent's. |
| **Checked out (hold)** | "Hang on, Ghost": the session stays, Ghost stops listening until addressed by name at the start of an utterance; nothing heard meanwhile is kept (ADR 0021). | Hung up. |
| **Hang up** | A goodbye ends the session; Ghost says a short goodbye and the desk returns to passive (ADR 0021). The End button on the phone ends it silently. | "Go to sleep" (gone). |
| **Passive** | The desk listening locally for the wake phrase only; nothing leaves the machine (ADR 0010). | |
| **Gate** | The one path for every change: the permissions' level, bounds, the yes when it asks, the call, the audit (ADR 0007, 0011, 0034). | |
| **Skill** | A `SKILL.md` folder: instructions the model reads on demand through `read_skill`, never a source of permission. Read nearest first: the department's, the operator's `~/.agents/skills`, Ghost's own. Scripts in it run only through the scripts app, a change through the gate (ADR 0023). | |
| **App** | Something connected to Ghost (GitHub, Linear, Notion): signed in to, given a key, or run on this computer; listed in `apps.json` and reached from every department (ADR 0034). Every tool it has is offered: a read-only one is a read, anything else a change through the gate. Also *source*: an app, the agents or the departments, as the gate and the audit know them (the audit's `ability` field holds the source's name). | A *server* or an *MCP server*: the protocol's words, never shown. An *ability* (ADR 0012, superseded). |
| **Connect / disconnect** | Add an app to Ghost from the catalog, by its address, or by pasting another tool's config; take it away again (its sign-in and the keys Ghost kept for it go too). | |
| **Sign in** | Connecting an app the way a site offers "Sign in with Google": Ghost opens the browser and keeps what comes back. When the app wants it again, it waits on the waiting list. | Typing a key, which is *use an API key*. |
| **Catalog** | The apps Ghost can connect, from its own copy of the public app registry, first-party ones first. Its text is shown to the operator, never to the model. | |
| **Permission** | A rule in `[permissions]` of `policy.toml`, or in a department's `department.json`: an app, a tool, or a tool with argument patterns, in one of `block`, `ask`, `quiet` or `allow`. The most careful match wins; what no rule names runs at the *mode* (`open`, `ask`, `read-only`). A department can only add care. | A *level* (`deny`, `confirm`, `announce`, `quiet`), the gate's own word for what a permission resolves to. |
| **Journal** | A department's writes in the audit, by any of its agents, newest first. | |
| **Watch** | What a department declares to observe an outside system: a read polled on an interval, or a signed request pushed to the daemon. It keeps typed fields and decides deterministically first, then with Jev, then a model, only where it declares them (ADR 0028). Never written by the model. | A routine, which runs the model. |
| **Observation** | What one look by a watch found: its typed fields, nothing else. Untrusted data. | |
| **Item** | Something a watch or Ghost itself is tracking, with a stable key, so one failure is one item however often it is seen. Resolved when its source says so. | |
| **Response** | What a watch does with its items: *record*, *tell* (put it on the waiting list) or *act* (run a routine) (ADR 0029). The operator's, never the model's. | A *rule*, which is a line of `AGENTS.md`. |
| **Waiting list** | The items that need the operator: what watches *tell*, changes waiting for a yes, apps that aren't connected or want a new sign-in, departments with a problem. "What needs me?" reads it (ADR 0029). | The audit, which is history. |
| **Route** | How Ghost reaches the operator about an item: the digest, desk voice, a desktop notification, the phone app, a call, a carrier. Which ones, and how loud, is the operator's policy (ADR 0031). | |
| **Delivered / acknowledged** | Delivered: a route passed the item on. Acknowledged: the operator heard it (replied, opened it, pressed acknowledge). Only acknowledged stops the alerting; a snooze stops it until a time. | |
| **Output** | A file Ghost made, in a department's `out/` folder, written through the gate (ADR 0033). Ghost never runs it. | |
| **Wisp** | Ghost's mascot and the only ghost drawing in the apps; an agent is shown as Wisp in the agent's colour. | |

Retired words, so old notes still read: a **line** was an agent; a **toolkit** was a
permission bundle in the policy file and a **template** a folder that stamped one out,
both replaced by the department folder; a **settled** line is a retired agent; a thread's
**scope**, a department's **allowlist** and its **owner** routing are gone (ADR 0034: a
department narrows with `block` rules); a **server** is an app, its **declaration** of
reads, writes and levels is gone, and a department no longer **uses** one of Ghost's.
