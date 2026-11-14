/**
 * The mock daemon's other tools on this computer (ADR 0068, `ghost_api::other_tools`). Three
 * shapes, from the scenario: none (today's first run), two tools with five apps (one runs a
 * command, one signs in, one is the same as another) and a local model, or a tool whose file
 * is malformed. Importing connects through the daemon's own connect; letting a tool use Ghost
 * makes a client as Settings › Outside tools does. Synthetic paths and names only.
 */
import type { FoundAppView, OtherImportResult, OtherLinkResult, OtherToolsView, OtherToolView } from "@ghost/client-ts";

import { HttpError } from "./errors";

export type OtherToolsMode = "none" | "found" | "malformed";

export interface OtherToolsHooks {
  /** Connect an app as a pasted `mcpServers` entry would; the daemon's own connect. */
  connect(name: string, entry: Record<string, unknown>, title: string): { name: string; sign_in_url: string | null };
  /** Make a client the way POST /clients does. */
  addClient(name: string): { name: string; title: string; created_at: string; last_used_at: string | null };
}

const app = (a: Partial<FoundAppView> & Pick<FoundAppView, "key" | "name" | "suggested" | "file">): FoundAppView => ({
  scope: "every project",
  kind: "address",
  address: null,
  command: [],
  command_line: null,
  sign_in: false,
  moved: [],
  needs: [],
  notes: [],
  skip: null,
  ...a,
});

function found(): OtherToolsView {
  const claude: OtherToolView = {
    id: "claude-code",
    title: "Claude Code",
    installed: true,
    files: [{ path: "~/.claude.json", state: "read", problem: null }],
    uses_ghost: null,
    link: {
      client: "claude-code",
      by: "command",
      shown: "claude mcp add --scope user ghost -- /usr/lib/heka-ghost-desktop/ghostd mcp serve --client claude-code",
      by_address: false,
    },
    link_problem: null,
    apps: [
      app({ key: "claude/github", name: "github", suggested: "github", file: "~/.claude.json", address: "https://api.githubcopilot.com/mcp/", moved: ["header Authorization"] }),
      app({ key: "claude/linear", name: "linear", suggested: "linear", file: "~/.claude.json", address: "https://mcp.linear.app/mcp", sign_in: true }),
      app({
        key: "claude/notes",
        name: "notes",
        suggested: "notes",
        file: "~/.claude.json",
        scope: "~/code/handbook only",
        kind: "command",
        command: ["npx", "-y", "@modelcontextprotocol/server-filesystem", "/home/you/notes"],
        command_line: "npx -y @modelcontextprotocol/server-filesystem /home/you/notes",
      }),
    ],
  };
  const cursor: OtherToolView = {
    id: "cursor",
    title: "Cursor",
    installed: true,
    files: [{ path: "~/.cursor/mcp.json", state: "read", problem: null }],
    uses_ghost: null,
    link: {
      client: "cursor",
      by: "file",
      shown: 'Adds to ~/.cursor/mcp.json:\n{\n  "mcpServers": {\n    "ghost": {\n      "type": "stdio",\n      "command": "/usr/lib/heka-ghost-desktop/ghostd",\n      "args": ["mcp", "serve", "--client", "cursor"]\n    }\n  }\n}',
      by_address: false,
    },
    link_problem: null,
    apps: [
      app({ key: "cursor/github", name: "github", suggested: "github-2", file: "~/.cursor/mcp.json", address: "https://api.githubcopilot.com/mcp", skip: "The same as github, found in Claude Code." }),
      app({ key: "cursor/sentry", name: "sentry", suggested: "sentry", file: "~/.cursor/mcp.json", address: "https://mcp.sentry.dev/mcp", sign_in: true }),
    ],
  };
  return {
    line: "Claude Code and Cursor are set up on this computer.",
    tools: [claude, cursor],
    models: [{ provider: "ollama", title: "Ollama", how: "Ollama is running on this computer, with 2 models.", models: ["llama3.2:latest", "qwen3:8b"] }],
    own_sign_in: ["Claude Code"],
  };
}

function malformed(): OtherToolsView {
  return {
    line: "Cursor is set up on this computer.",
    tools: [
      {
        id: "cursor",
        title: "Cursor",
        installed: true,
        files: [{ path: "~/.cursor/mcp.json", state: "unreadable", problem: "~/.cursor/mcp.json could not be read as JSON, so Ghost left it as it is." }],
        apps: [],
        uses_ghost: null,
        link: null,
        link_problem: "~/.cursor/mcp.json could not be read, so Ghost leaves it alone.",
      },
    ],
    models: [],
    own_sign_in: [],
  };
}

export class MockOtherTools {
  private view: OtherToolsView;

  constructor(
    mode: OtherToolsMode,
    private hooks: OtherToolsHooks,
  ) {
    this.view = mode === "found" ? found() : mode === "malformed" ? malformed() : { line: null, tools: [], models: [], own_sign_in: [] };
  }

  handle(method: string, path: string, body: Record<string, unknown>): unknown | undefined {
    if (method === "GET" && path === "/other-tools") return structuredClone(this.view);
    const r = /^\/other-tools\/([^/]+)\/(import|link)$/.exec(path);
    if (!r) return undefined;
    const tool = this.view.tools.find((t) => t.id === r[1]);
    if (!tool) throw new HttpError(404, `no tool called ${r[1]}`);
    if (r[2] === "import" && method === "POST") return this.importApps(tool, (body.apps as string[]) ?? []);
    if (r[2] === "link" && method === "POST") return this.link(tool);
    if (r[2] === "link" && method === "DELETE") return this.unlink(tool);
    return undefined;
  }

  private importApps(tool: OtherToolView, keys: string[]): OtherImportResult {
    let url: string | null = null;
    const skipped: OtherImportResult["skipped"] = [];
    for (const key of keys) {
      const a = tool.apps.find((x) => x.key === key);
      if (!a) throw new HttpError(404, `${tool.title} has no app ${JSON.stringify(key)} now; look again`);
      if (a.skip) {
        skipped.push({ key, name: a.name, why: a.skip });
        continue;
      }
      const entry = a.kind === "command" ? { command: a.command[0], args: a.command.slice(1) } : { url: a.address };
      const made = this.hooks.connect(a.suggested, entry, a.name[0].toUpperCase() + a.name.slice(1));
      url ??= a.sign_in ? made.sign_in_url : null;
      a.skip = `Already connected, as ${made.name}.`;
    }
    return { apps: [], skipped, sign_in_url: url, tools: structuredClone(this.view) };
  }

  private link(tool: OtherToolView): OtherLinkResult {
    if (tool.uses_ghost || !tool.link) throw new HttpError(409, tool.link_problem ?? `${tool.title} already uses Ghost.`);
    const client = this.hooks.addClient(tool.link.client);
    const backup = tool.link.by === "file" ? `${tool.files[0].path}.ghost-backup-20261114T120000` : null;
    tool.uses_ghost = { entry: "ghost", client: client.name, file: tool.files[0].path, removable: true };
    tool.link = null;
    return { done: `${tool.title} can use Ghost now; restart it if it is open.`, client, backup, tools: structuredClone(this.view) };
  }

  private unlink(tool: OtherToolView): OtherLinkResult {
    if (!tool.uses_ghost) throw new HttpError(404, `${tool.title} does not use Ghost.`);
    const client = tool.uses_ghost.client ?? tool.id;
    tool.uses_ghost = null;
    tool.link = {
      client,
      by: tool.id === "cursor" ? "file" : "command",
      shown: tool.id === "cursor" ? "Adds to ~/.cursor/mcp.json" : `claude mcp add --scope user ghost -- ghostd mcp serve --client ${client}`,
      by_address: false,
    };
    return { done: `${tool.title} no longer uses Ghost.`, client: null, backup: null, tools: structuredClone(this.view) };
  }
}
