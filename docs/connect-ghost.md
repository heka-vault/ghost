# Connecting other tools to Ghost

Claude Code, Cursor, Codex and other tools on this computer can use Ghost: tell you
something by voice or on your waiting list ("tell me when the build is done"), read
Ghost's status and documentation, and wait for you to acknowledge what they sent. Ghost
calls them *outside tools*. Each one you connect has a name and a token of its own, so you
hear who a message is from ("From Claude Code: the build finished") and can remove one
without the others (ADR 0048).

## Connect a tool

In the desktop app: **Settings › Outside tools › Connect a tool**. Give it a name (Claude
Code, Cursor) and Ghost shows its token once, with the line to paste into the tool.

Or from a terminal:

```
ghostd mcp add claude-code
```

A name is lowercase letters, digits and hyphens. The command prints the token and the two
snippets below with the token filled in. Ghost keeps only a fingerprint of the token and
cannot show it again; if you lose it, remove the tool and connect it again.

### Claude Code

```
claude mcp add --transport http ghost http://127.0.0.1:47824/mcp --header "Authorization: Bearer <token>"
```

### Cursor

Add Ghost to `~/.cursor/mcp.json` (or a project's `.cursor/mcp.json`):

```json
{
  "mcpServers": {
    "ghost": {
      "type": "http",
      "url": "http://127.0.0.1:47824/mcp",
      "headers": { "Authorization": "Bearer <token>" }
    }
  }
}
```

### Any other tool

Give it the address `http://127.0.0.1:47824/mcp` (Streamable HTTP) and the header
`Authorization: Bearer <token>`. Most tools read the same `mcpServers` block as Cursor.

### A tool that can only start a command

Some tools cannot connect to an address; they start a program and talk to it. For those:

```
ghostd mcp add codex --command
```

Ghost writes the token to `~/.local/share/ghost/clients/codex.token`, readable only by
you, and the tool's config holds a command and no token:

```json
{
  "mcpServers": {
    "ghost": { "command": "ghostd", "args": ["mcp", "serve", "--client", "codex"] }
  }
}
```

For Claude Code that is `claude mcp add ghost -- ghostd mcp serve --client codex`.
`ghostd mcp add` prints the full path of `ghostd` in place of the bare name.
`ghostd mcp serve` forwards to the running Ghost. If Ghost is not up yet it waits 15
seconds, and it carries on across a restart of Ghost.

## See and remove them

```
ghostd mcp list
ghostd mcp remove claude-code
```

Settings › Outside tools shows the same list with when each was last used. A removed tool
is refused from its next request.

## What a tool can do

| Tool | What it does |
|---|---|
| `notify(title, detail?, urgency?)` | Puts an item on your waiting list; Ghost's routes bring it to you (the digest, desk voice, a desktop notification, your phone) |
| `resolve(key)` | Clears an item that tool put there |
| `say(text)` | Ghost says one sentence at your desk now, if the desk is free and you let Ghost speak first; otherwise it is an item on the waiting list |
| `events_wait(since?, timeout_s?)` | Waits up to a minute for something to change, such as you acknowledging its item |
| `status()`, `apps_list()`, `audit_recent()` | How Ghost is, the connected apps, recent changes |
| `docs_search(query)`, `docs_read(name, section?)` | Ghost's documentation; also as `ghost://docs/<name>`, and `ghost://status` |
| `agents_list()` | Your agents and their jobs |
| `agent_ask(agent, text)` | A written question for an agent, and its written answer. The agent works as it does on a schedule: its own memory and tools, nothing spoken, ten minutes at the most. A change your permissions ask about is not made; it waits on your waiting list. The question and the answer are kept in that agent's transcript, marked as asked by the tool |
| `schedules_list(id?)`, `schedule_manage(action, …)` | The schedules, and setting one up: an instruction an agent carries out on its own at a time. What a tool sets up is marked as its own and runs as the agent it names (Ghost when it names none); it changes, pauses and removes only its own, and of the others sees only that they are there |
| `waiting_ack(keys)`, `waiting_snooze(keys, minutes)` | Marks items on the waiting list as heard, or quiet for a while |

`say` speaks only when desk voice is on: `desk` in `[routes].enabled` of
`~/.config/ghost/policy.toml` (Settings › Notifications), outside your quiet hours and
do-not-disturb, with no session, hold or call under way.

A tool cannot read your passwords, the policy file, a transcript or anything Ghost hears,
and cannot change a permission, an app or another tool.

## Your permissions apply

Every change a tool makes goes through the gate as `ghost.<tool>` and is in the audit log
under the tool's name (the session `client:<name>`). The rules in `[permissions]` of
`policy.toml` hold for it as for an agent:

```toml
[permissions]
block = ["ghost.say"]            # no tool may make Ghost speak
ask   = ["ghost.waiting_ack"]    # ask me before a tool marks things as heard
```

A change you are asked about waits on your waiting list; the tool is told "waiting for the
operator" and nothing runs until you say yes. Each tool may make 60 calls a minute, 10 of
which may tell you something.

## Where Ghost listens

On this computer only: `127.0.0.1:47824`. To change it, set `GHOST_MCP_LISTEN` in
`~/.config/ghost/settings.env` (`host:port`, comma separated) and restart Ghost; an empty
value turns it off. An address that is not this computer sends the token over that network
unencrypted, so use only your own WireGuard network for it. A port that is taken shows on
your waiting list, and `ghostd mcp add` still works; the tool connects once Ghost listens.
