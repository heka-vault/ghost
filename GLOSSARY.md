# Glossary

The words Ghost uses, one meaning each. The operator hires an **agent** and talks to it;
what is said is the **conversation**. The phone metaphor of the first increment (*line*,
*patch me through*) is gone (ADR 0024), and so are departments (ADR 0035).

| Term | Meaning | Not |
|---|---|---|
| **Agent** | What the operator hires, talks to and retires: its own name, job, persona, voice, private memory, notes and transcript, in `~/.config/ghost/agents/<slug>/`. Ghost is agent zero. The desktop's Agents screen and the phone's Home list them. The code and the control API call the same object a *conversation* (`ghost_core.conversations`, `/conversations`); the word to show is **agent**. | A session. |
| **Conversation** | What is said with an agent: its transcript, with the agent's memory of it. Opening an agent in the desktop shows the conversation first; the agent's settings sit behind it. In code, also the storage name for the agent. | |
| **Session** | One stretch with the mic open, from a wake or a call to a hang-up. A session starts in one agent (Ghost at the desk; on the phone, Ghost or the agent you called) and can be transferred to another. | |
| **Call** | A phone session, started from the phone's Home: the ghost for Ghost, a key for an agent. | |
| **Transfer / switch** | Moving a running session to another agent; "back to Ghost" returns. Retired agents are out of reach by voice. | |
| **Hire / retire / rehire** | Create an agent; take it out of voice routing and the active list, transcript and audit kept; bring it back. Delete removes it for good. | |
| **Rule** | A line of Ghost's `AGENTS.md`, in every agent's prompt. "Add a rule" appends the operator's words verbatim through the gate. | A persona, which is one agent's. |
| **Checked out (hold)** | "Hang on, Ghost": the session stays, Ghost stops listening until addressed by name at the start of an utterance; nothing heard meanwhile is kept (ADR 0021). | Hung up. |
| **Hang up** | A goodbye ends the session; Ghost says a short goodbye and the desk returns to passive (ADR 0021). The End button on the phone ends it silently. | "Go to sleep" (gone). |
| **Passive** | The desk listening locally for the wake phrase only; nothing leaves the machine (ADR 0010). | |
| **Gate** | The one path for every change: the permissions' level, bounds, the yes when it asks, the call, the audit (ADR 0007, 0011, 0034). | |
| **Skill** | A `SKILL.md` folder: instructions the model reads on demand through `read_skill`, never a source of permission. Ghost's own `skills/`, then the operator's `~/.agents/skills`. Scripts in it run only through the scripts app, a change through the gate (ADR 0023). | |
| **App** | Something connected to Ghost (GitHub, Linear, Notion): signed in to, given a key, or run on this computer; listed in `apps.json` and reached by every agent (ADR 0034). Every tool it has is offered: a read-only one is a read, anything else a change through the gate. Also *source*: an app, the agents, the rules or the outputs, as the gate and the audit know them (the audit's `source` field). | A *server* or an *MCP server*: the protocol's words, never shown. An *ability* (ADR 0012, superseded). |
| **Connect / disconnect** | Add an app to Ghost from the catalog, by its address, or by pasting another tool's config; take it away again (its sign-in and the keys Ghost kept for it go too). | |
| **Sign in** | Connecting an app the way a site offers "Sign in with Google": Ghost opens the browser and keeps what comes back. When the app wants it again, it waits on the waiting list. | Typing a key, which is *use a password*. |
| **Password** | Anything secret Ghost holds for the operator: an API key, a token, an account password. Kept in Ghost's own vault, encrypted, referred to by name (`${GITHUB_TOKEN}`) and filled in only where it is used: an app's environment, a request header, the model connection. A password goes in and never comes back out: not to the desktop, the control API, a log, the audit, a transcript or the model, and an app that echoes one gets it replaced before anything reads the result. The vault is one encrypted file, its key in the operator's keyring (ADR 0036); the old plain files (`~/.config/ghost/env`, `apps.env`) and heka-vault are imported once; Ghost then deletes the plain files (keeping them if a value couldn't be imported, or one differs from the vault's copy), and heka-vault's own config (`~/.config/heka-vault/ghost.env`) is left for the operator to delete. | A *secret*, *key*, *API key* or *token*: the engineering words, kept in code, never shown. An `.env` file. |
| **Catalog** | The apps Ghost can connect, from its own copy of the public app registry, first-party ones first. Its text is shown to the operator, never to the model. | |
| **Permission** | A rule in `[permissions]` of `policy.toml`: an app, a tool, or a tool with argument patterns, in one of `block`, `ask`, `quiet` or `allow`. The most careful match wins; what no rule names runs at the *mode* (`open`, `ask`, `read-only`). There is one set; nothing narrows it per agent (ADR 0035). | A *level* (`deny`, `confirm`, `announce`, `quiet`), the gate's own word for what a permission resolves to. |
| **Watch** | What the operator declares in `watches.json` (or `hooks.json`) to observe an outside system: a read polled on an interval, or a signed request pushed to the daemon. It keeps typed fields and decides deterministically first, then with Jev, then a model, only where it declares them (ADR 0028). Never written by the model. | A routine, which runs the model. |
| **Observation** | What one look by a watch found: its typed fields, nothing else. Untrusted data. | |
| **Item** | Something a watch or Ghost itself is tracking, with a stable key, so one failure is one item however often it is seen. Resolved when its source says so. | |
| **Response** | What a watch does with its items: *record*, *tell* (put it on the waiting list) or *act* (run a routine) (ADR 0029). The operator's, never the model's. | A *rule*, which is a line of `AGENTS.md`. |
| **Waiting list** | The items that need the operator: what watches *tell*, changes waiting for a yes, apps that aren't connected or want a new sign-in, files in Ghost's folder with a problem. "What needs me?" reads it (ADR 0029). | The audit, which is history. |
| **Route** | How Ghost reaches the operator about an item: the digest, desk voice, a desktop notification, the phone app, a call, a carrier. Which ones, and how loud, is the operator's policy (ADR 0031). | |
| **Delivered / acknowledged** | Delivered: a route passed the item on. Acknowledged: the operator heard it (replied, opened it, pressed acknowledge). Only acknowledged stops the alerting; a snooze stops it until a time. | |
| **Output** | A file Ghost made, in `~/.config/ghost/out/`, written through the gate (ADR 0033). Ghost never runs it. | |
| **Wisp** | Ghost's mascot and the only ghost drawing in the apps; an agent is shown as Wisp in the agent's colour. | |

Retired words, so old notes still read: a **line** was an agent; a **toolkit** was a
permission bundle in the policy file and a **template** a folder that stamped one out; a
**department** was a folder of rules, skills, memory and agents, with its own permissions,
and a **move** put an agent in another one (ADR 0024, gone with ADR 0035, as are its
**journal** and shared memory); a **settled** line is a retired agent; a thread's
**scope**, an **allowlist** and **owner** routing are gone (ADR 0034); a **server** is an
app, and its **declaration** of reads, writes and levels is gone.
