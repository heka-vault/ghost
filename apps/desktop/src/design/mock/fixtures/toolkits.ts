/**
 * Owner: toolkits. The `[toolkit.<name>]` tables in the policy file (ADR 0023), as GET
 * /toolkits reports them: what each grants per server (null lists mean everything the server
 * offers), the skills it carries, and what is wrong with it against the file. The problems
 * are worked out here with the same rules ghost_core.policy.check_toolkits applies, so the
 * mock's PUT answers as the daemon does: a looser level, a server or write the file doesn't
 * declare, an item outside the server's scope, a skill that isn't installed.
 */
import type { GrantView, ToolkitUpdate, ToolkitView } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";
import type { ServerFixture } from "./mcp";
import type { SkillFixture } from "./skills";

/** Least to most permissive: a toolkit's level may sit at or left of the policy's. */
export const LEVELS = ["deny", "confirm", "announce", "quiet"] as const;

const grant = (server: string, g: Partial<Omit<GrantView, "server">> = {}): GrantView => ({ server, reads: null, writes: null, items: null, levels: {}, ...g });

/** The toolkits by name; each scenario picks from these, and a template names them (fixtures/templates.ts). */
export const TOOLKIT_SEEDS: Record<string, Omit<ToolkitView, "problems">> = {
  finance: {
    name: "finance",
    description: "The books: billing issues in the app repo, invoices, and the month close.",
    skills: ["month-close", "invoicing"],
    grants: [grant("github_heka", { writes: ["comment", "set_labels"], items: ["ghost-mock/app"], levels: { set_labels: "announce" } })],
  },
  code: {
    name: "code",
    description: "Everything the heka GitHub offers, at the levels in the policy file.",
    skills: ["triage"],
    grants: [grant("github_heka")],
  },
  // A toolkit the operator wrote by hand: it tries to loosen `close` and names a skill that isn't installed here.
  ops: {
    name: "ops",
    description: "On-call: the infra repo, read mostly, and the notes vault.",
    skills: ["standup"],
    grants: [grant("github_heka", { reads: ["issue_read", "list_issues"], writes: ["comment", "close"], items: ["ghost-mock/infra"], levels: { close: "quiet", comment: "confirm" } }), grant("notes")],
  },
  design: {
    name: "design",
    description: "Design references from Mobbin, nothing that writes.",
    skills: ["design-review"],
    grants: [grant("mobbin")],
  },
  client: {
    name: "client",
    description: "Acme’s repositories: comments and labels on the app, nothing on the public one.",
    skills: [],
    grants: [grant("github_work", { writes: ["comment", "set_labels"], items: ["acme/app"] })],
  },
};

const NORMAL = ["finance", "code", "ops"];
const BUSY = ["finance", "code", "ops", "design", "client"];

/**
 * What is wrong with one toolkit against the file, worded as ghost_core.policy.check_toolkits
 * words it. The ceiling for a write is the level the policy resolves for it on the server
 * (`McpWriteInfo.autonomy`, which the mock re-reads from the file on every PUT /policy).
 */
export function problemsOf(name: string, t: Pick<ToolkitUpdate, "grants" | "skills">, servers: ServerFixture[], skillNames: Set<string>): string[] {
  const out: string[] = [];
  for (const g of t.grants) {
    const spec = servers.find((f) => f.info.name === g.server)?.info;
    if (!spec) {
      out.push(`toolkit ${name}: server ${g.server} is not declared`);
      continue;
    }
    if (g.items != null) {
      if (!spec.scope) out.push(`toolkit ${name}: ${g.server} has no scope to narrow`);
      else {
        const allowed = new Set(spec.scope.items.map((i) => i.toLowerCase()));
        for (const item of g.items) if (!allowed.has(item.toLowerCase())) out.push(`toolkit ${name}: ${item} is not in ${g.server}'s scope items`);
      }
    }
    if (g.writes != null) for (const op of g.writes) if (!spec.writes.some((w) => w.operation === op)) out.push(`toolkit ${name}: ${g.server} declares no write ${op}`);
    for (const [op, level] of Object.entries(g.levels)) {
      const ceiling = spec.writes.find((w) => w.operation === op)?.autonomy ?? "confirm";
      if (LEVELS.indexOf(level as (typeof LEVELS)[number]) > LEVELS.indexOf(ceiling as (typeof LEVELS)[number])) {
        out.push(`toolkit ${name}: ${g.server}.${op} = ${level} is looser than the file's ${ceiling}; a toolkit only narrows`);
      }
    }
  }
  for (const sk of t.skills) if (!skillNames.has(sk)) out.push(`toolkit ${name}: no skill named ${sk}`);
  return out;
}

/** The TOML Ghost writes for one toolkit, as ghost_core.toolkits.render_toolkits does. */
export function renderToolkit(t: Omit<ToolkitView, "problems">): string {
  const q = (s: string) => JSON.stringify(s);
  const key = (k: string) => (/^[A-Za-z0-9_-]+$/.test(k) ? k : q(k));
  const lines = [`[toolkit.${key(t.name)}]`];
  if (t.description) lines.push(`description = ${q(t.description)}`);
  if (t.skills.length) lines.push(`skills = [${t.skills.map(q).join(", ")}]`);
  for (const g of t.grants) {
    lines.push("", `[toolkit.${key(t.name)}.servers.${key(g.server)}]`);
    if (g.reads != null) lines.push(`reads = [${g.reads.map(q).join(", ")}]`);
    if (g.writes != null) lines.push(`writes = [${g.writes.map(q).join(", ")}]`);
    if (g.items != null) lines.push(`items = [${g.items.map(q).join(", ")}]`);
    const levels = Object.entries(g.levels);
    if (levels.length) lines.push(`levels = { ${levels.map(([op, lv]) => `${key(op)} = ${q(lv)}`).join(", ")} }`);
  }
  return lines.join("\n") + "\n";
}

export default function toolkits(s: Scenario, servers: ServerFixture[], skills: SkillFixture[]): ToolkitView[] {
  if (s.data === "empty") return [];
  const names = new Set(skills.map((f) => f.info.name));
  return (s.data === "busy" ? BUSY : NORMAL).map((n) => {
    const t = structuredClone(TOOLKIT_SEEDS[n]);
    return { ...t, problems: problemsOf(t.name, t, servers, names) };
  });
}
