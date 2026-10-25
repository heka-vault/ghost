# Glossary

> **Changing (ADR 0024, accepted 2026-10-25, not yet built).** When the department
> work lands, three words replace the phone metaphor below: an **agent** is what the
> operator hires, talks to, moves and fires (its own persona, job, voice and private
> memory); a **department** is the folder it works in (rules, servers, skills, docs and a
> shared memory every agent in it reads); **conversation** stays as the storage name for
> an agent in the code and the control API. *Line*, *toolkit* and *template* go. The
> table below describes what runs today and is rewritten with the code.

The words Ghost uses, one meaning each. In the apps and in speech the phone metaphor
is the one that holds: you call a line, and what is said on it is the conversation.

| Term | Meaning | Not |
|---|---|---|
| **Line** | A named thing you can call: its own name, voice, persona, memory, scope and transcript. Ghost is line zero and unscoped; every other line is scoped (ADR 0019). The desktop's Lines screen and the phone's dial pad list them. The code and ADRs 0014/0019 call the same object a *conversation* (`ghost_core.conversations`, `/conversations` in the control API); the operator also says *thread* and *department*. Those are the same thing; the word to show is **line**. | A session. |
| **Conversation** | What is said on a line: its transcript, with Ghost's memory of it. Opening a line in the desktop app shows the conversation first; the line's settings sit behind the More menu. | The line itself (see above). |
| **Session** | One stretch with the mic open, from a wake or a call to a hang-up. A session starts in one line (Ghost at the desk; on the phone, Ghost or the line you called) and can be patched to another. | |
| **Call** | A phone session, started from the phone's Home: the ghost for Ghost, a key on the dial pad for a line. | |
| **Patch (me) through / switch** | Moving a running session to another live line; "back to Ghost" returns. Settled lines are out of reach by voice. | |
| **Settled** | A line that is done: out of voice routing and the active list, still readable, can be unsettled (ADR 0019). | Deleted. |
| **Scope** | What a line may touch per server (for GitHub, repositories); the gate refuses writes outside it (ADR 0019). | |
| **Checked out (hold)** | "Hang on, Ghost": the session stays, Ghost stops listening until addressed by name at the start of an utterance; nothing heard meanwhile is kept (ADR 0021). | Hung up. |
| **Hang up** | A goodbye ends the session; Ghost says a short goodbye and the desk returns to passive (ADR 0021). The End button on the phone ends it silently. | "Go to sleep" (gone). |
| **Passive** | The desk listening locally for the wake phrase only; nothing leaves the machine (ADR 0010). | |
| **Gate** | The one path for every write: policy level, snapshot, bounds, read-back, audit (ADR 0007, 0011). | |
| **Department** | A line that has toolkits: its sessions reach only what they grant (none: everything, like Ghost). It is its own primary line, and topic lines can be started in it (ADR 0023). "Transfer me to accounting" lands on it. | Ghost, which is unscoped. |
| **Toolkit** | A named bundle of permissions a department is given: which servers (which tools, which items, at what level, never looser than the policy file) and which skills (ADR 0023). Designed in the desktop, stored in the policy file. | An *ability* (the retired packages). |
| **Skill** | A `SKILL.md` folder: instructions the model reads on demand through `read_skill`, never a source of permission. Scripts in it run only through the gated scripts server (ADR 0023). | |
| **Template** | A folder that creates a department with its toolkits and skills in one step. The operator's earlier word was *workspace*, now dropped (ADR 0023). | A live department. |
| **Server** | An MCP server the operator declares in the policy file: its reads are offered to the model, its writes by declaration through the gate (ADR 0022). Also *source*: a server or the lines, as the gate and the audit know them (the audit's `ability` field holds the source's name). | An *ability*: the installable packages Ghost had until 2026-10-20 (ADR 0012, superseded). |
| **Wisp** | Ghost's mascot and the only ghost drawing in the apps; a line is shown as Wisp in the line's colour. | |
