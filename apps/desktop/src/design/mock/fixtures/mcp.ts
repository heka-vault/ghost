/**
 * Owner: servers. The MCP servers the policy file declares (ADR 0022), as GET /mcp reports
 * them: connected or not, every tool the server has (reads are `offered`), the writes the
 * operator declared for the gate, and the scope a line can be limited to. Each server also
 * carries the TOML of its own declaration, so the policy fixture can render a file that
 * declares the same servers the screens show.
 */
import type { McpServerInfo, McpWriteInfo } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";

export interface ServerFixture {
  info: McpServerInfo;
  /** The `[mcp.<name>]` declaration (transport, reads, scope, writes), without levels or targets. */
  toml: string;
  /** The server's one-paragraph prompt, as a source's description (its first sentence). */
  prompt: string;
}

const ISSUE = { owner: { type: "string" }, repo: { type: "string" }, issue_number: { type: "integer" } };
const write = (operation: string, tool: string, risk: string, autonomy: string, asked_by: string[], extra: Record<string, unknown> = {}, required: string[] = [], read: string | null = "issue_read"): McpWriteInfo => ({
  operation,
  tool,
  risk,
  autonomy,
  asked_by,
  target: ["owner", "repo", "issue_number"],
  read,
  offered: autonomy !== "deny",
  properties: { ...ISSUE, ...extra },
  required: ["owner", "repo", "issue_number", ...required],
});

/** The official GitHub server (github-mcp-server, issues toolset), one token for the heka organisation. */
function githubHeka(repos: string[]): ServerFixture {
  const items = repos.map((r) => JSON.stringify(r)).join(", ");
  return {
    prompt: "Through github_heka you can look things up in the heka repositories and, with a declaration, label, comment on, close and reopen issues.",
    info: {
      name: "github_heka",
      transport: "stdio",
      connected: true,
      error: null,
      department: null,
      tools: [
        { name: "issue_read", description: "Get details of a specific issue in a GitHub repository: title, body, state, labels, assignees, comments and sub-issues.", offered: true },
        { name: "list_issues", description: "List issues in a GitHub repository. Filter by state, labels, since and ordering; paginated.", offered: true },
        { name: "search_issues", description: "Search for issues in GitHub repositories using issues search syntax already scoped to is:issue.", offered: true },
        { name: "issue_write", description: "Create a new issue or update an existing one in a GitHub repository: title, body, state, labels, assignees, milestone.", offered: false },
        { name: "add_issue_comment", description: "Add a comment to a specific issue in a GitHub repository.", offered: false },
        { name: "assign_copilot_to_issue", description: "Assign Copilot to a specific issue in a GitHub repository. It opens a pull request for the issue.", offered: false },
        { name: "list_issue_types", description: "List supported issue types for a repository owner (organization).", offered: false },
      ],
      writes: [
        // Levels mirror fixtures/policy.ts skills: the mock re-applies them on every PUT /policy,
        // so the screen shows the same truth before and after a save.
        write("comment", "add_issue_comment", "low", "announce", ["comment", "reply", "note"], { body: { type: "string" } }, ["body"]),
        write("set_labels", "issue_write", "low", "quiet", ["label", "tag", "mark"], { labels: { type: "array", items: { type: "string" } } }, ["labels"]),
        write("close", "issue_write", "high", "confirm", ["close", "shut"], { state_reason: { type: "string", enum: ["completed", "not_planned", "duplicate"] } }),
        write("reopen", "issue_write", "medium", "confirm", ["reopen", "open"]),
      ],
      scope: { kind: "repository", arguments: ["owner", "repo"], items: [...repos] },
    },
    toml: `[mcp.github_heka]
command = "github-mcp-server"
args = ["stdio", "--toolsets", "issues"]
env = { GITHUB_PERSONAL_ACCESS_TOKEN = "$GITHUB_TOKEN" }
reads = ["issue_read", "list_issues", "search_issues"]
scope = { kind = "repository", arguments = ["owner", "repo"], items = [${items}] }

[mcp.github_heka.writes.comment]
tool = "add_issue_comment"
args = ["owner", "repo", "issue_number", "body"]
target = ["owner", "repo", "issue_number"]
read = { tool = "issue_read", with = { method = "get" } }
asked_by = ["comment", "reply", "note"]
risk = "low"
key_in = "body"
description = "Add a comment to an issue."

[mcp.github_heka.writes.set_labels]
tool = "issue_write"
with = { method = "update" }
args = ["owner", "repo", "issue_number", "labels"]
target = ["owner", "repo", "issue_number"]
read = { tool = "issue_read", with = { method = "get" } }
asked_by = ["label", "tag", "mark"]
risk = "low"
bound = { labels = 10 }
description = "Set an issue's labels to the full new list (read it first: labels replaces them all)."

[mcp.github_heka.writes.close]
tool = "issue_write"
with = { method = "update", state = "closed" }
args = ["owner", "repo", "issue_number", "state_reason"]
target = ["owner", "repo", "issue_number"]
read = { tool = "issue_read", with = { method = "get" } }
asked_by = ["close", "shut"]
risk = "high"
description = "Close an issue (state_reason: completed, not_planned or duplicate)."

[mcp.github_heka.writes.reopen]
tool = "issue_write"
with = { method = "update", state = "open" }
args = ["owner", "repo", "issue_number"]
target = ["owner", "repo", "issue_number"]
read = { tool = "issue_read", with = { method = "get" } }
asked_by = ["reopen", "open"]
risk = "medium"
description = "Reopen a closed issue."
`,
  };
}

/** The fleet's MCP proxy in front of Mobbin: reads only, over HTTP. */
const MOBBIN: ServerFixture = {
  prompt: "Through mobbin you can look up real app screens and flows for design references.",
  info: {
    name: "mobbin",
    transport: "http",
    connected: true,
    error: null,
    tools: [
      { name: "search_screens", description: "Search Mobbin for UI screens using natural language. Returns matching screens with inline images and metadata.", offered: true },
      { name: "search_flows", description: "Search Mobbin for multi-step user flows (e.g. onboarding, checkout) using natural language.", offered: true },
      { name: "search_sections", description: "Search Mobbin for website sections (e.g. About, Pricing, Footer) using natural language.", offered: true },
    ],
    writes: [],
    scope: null,
  },
  toml: `[mcp.mobbin]
url = "http://proxy.lan:8318/mobbin/mcp"
headers = { Authorization = "Bearer $MCP_PROXY_KEY" }
`,
};

/** A notes server whose command isn't installed: declared, but Ghost couldn't start it. */
const NOTES: ServerFixture = {
  prompt: "Through notes you can read and write your notes.",
  info: {
    name: "notes",
    transport: "stdio",
    connected: false,
    error: "notes-mcp: command not found (looked on PATH and in ~/.local/bin)",
    tools: [],
    writes: [write("add_note", "create_note", "low", "announce", ["note", "jot", "remember"], { text: { type: "string" } }, ["text"], null)],
    scope: null,
  },
  toml: `[mcp.notes]
command = "notes-mcp"
args = ["--vault", "~/notes"]

[mcp.notes.writes.add_note]
tool = "create_note"
args = ["text"]
target = ["text"]
asked_by = ["note", "jot", "remember"]
risk = "low"
description = "Add a note to today's page."
`,
};

/** A second GitHub, for a client's organisation, with its own token and repositories (busy only). */
const GITHUB_WORK: ServerFixture = (() => {
  const base = githubHeka(["acme/app", "acme/public-app"]);
  return {
    prompt: "Through github_work you can look things up in Acme's repositories and, with a declaration, label, comment on, close and reopen issues.",
    info: { ...base.info, name: "github_work" },
    toml: base.toml.replace(/github_heka/g, "github_work").replace("$GITHUB_TOKEN", "$ACME_TOKEN"),
  };
})();

/** A calendar over the proxy, with one write declared and no read to check it against (busy only). */
const CALENDAR: ServerFixture = {
  prompt: "Through calendar you can see today's events and, with a declaration, add one.",
  info: {
    name: "calendar",
    transport: "http",
    connected: true,
    error: null,
    tools: [
      { name: "today", description: "Today's events with times and attendees.", offered: true },
      { name: "next_free_slot", description: "The next free half hour today.", offered: true },
      { name: "create_event", description: "Create an event on the primary calendar.", offered: false },
      { name: "delete_event", description: "Delete an event by id.", offered: false },
    ],
    writes: [{ operation: "create_event", tool: "create_event", risk: "medium", autonomy: "confirm", asked_by: ["schedule", "book", "add"], target: ["title", "start"], read: null, offered: true, properties: { title: { type: "string" }, start: { type: "string" }, end: { type: "string" } }, required: ["title", "start"] }],
    scope: null,
  },
  toml: `[mcp.calendar]
url = "http://proxy.lan:8318/calendar/mcp"
headers = { Authorization = "Bearer $MCP_PROXY_KEY" }
reads = ["today", "next_free_slot"]

[mcp.calendar.writes.create_event]
tool = "create_event"
args = ["title", "start", "end"]
target = ["title", "start"]
asked_by = ["schedule", "book", "add"]
risk = "medium"
description = "Create an event on the primary calendar."
`,
};

/** The repositories the heka GitHub server may touch: the allowlist a line's scope picks from. */
export const REPOS = ["ghost-mock/app", "ghost-mock/infra"];
export const BUSY_REPOS = [...REPOS, "ghost-mock/docs", "heka-vault/ghost"];

export default function mcp(s: Scenario): ServerFixture[] {
  // No policy file yet (the empty and first-run scenarios): nothing declared, nothing connected.
  if (s.data === "empty") return [];
  const base = [githubHeka(s.data === "busy" ? BUSY_REPOS : REPOS), MOBBIN, NOTES];
  return structuredClone(s.data === "busy" ? [base[0], GITHUB_WORK, CALENDAR, base[1], base[2]] : base);
}
