/** Owner: policy. The autonomy policy file as the daemon reports it. */
import type { PolicyView } from "@ghost/client-ts";

import type { ServerFixture } from "./mcp";
import type { Scenario } from "../scenarios";

/** ghost_core.policy.to_toml’s first line. */
const HEADER = "# Ghost autonomy policy (ADR 0011, ADR 0022). Edited through the desktop app or by hand.";

/** A TOML key: bare when it can be, quoted otherwise (`"*"`, `"ghost-mock/app"`). */
const key = (k: string) => (/^[A-Za-z0-9_-]+$/.test(k) ? k : JSON.stringify(k));

/**
 * The text Ghost writes for a policy, mirroring ghost_core.policy.to_toml: [defaults], then
 * each declared server (its `[mcp.<name>]` declaration as the file had it, then its levels
 * and target tables), then every other source in name order with its target tables right
 * after it. No comments: a structured save rewrites the file without them.
 */
export function renderToml(p: Pick<PolicyView, "default" | "skills" | "targets">, servers: ServerFixture[]): string {
  const lines = [HEADER, ""];
  if (p.default) lines.push("[defaults]", `autonomy = "${p.default}"`, "");
  const names = new Set(servers.map((f) => f.info.name));
  const levels = (table: string, ops: Record<string, string> | undefined) => {
    if (!ops || !Object.keys(ops).length) return;
    lines.push(`[${table}]`);
    for (const [op, lv] of Object.entries(ops)) lines.push(`${key(op)} = "${lv}"`);
    lines.push("");
  };
  const targetTables = (table: string, ts: Record<string, Record<string, string>> | undefined) => {
    for (const [target, ops] of Object.entries(ts ?? {})) levels(`${table}.targets."${target}"`, ops);
  };
  for (const f of servers) {
    lines.push(f.toml.trimEnd(), "");
    levels(`mcp.${key(f.info.name)}.levels`, p.skills[f.info.name]);
    targetTables(`mcp.${key(f.info.name)}`, p.targets[f.info.name]);
  }
  for (const skill of [...new Set([...Object.keys(p.skills), ...Object.keys(p.targets)])].filter((n) => !names.has(n)).sort()) {
    levels(key(skill), p.skills[skill] ?? {});
    targetTables(key(skill), p.targets[skill]);
  }
  return lines.join("\n").trimEnd() + "\n";
}

/** A file the operator has been into by hand: a few comments that a TOML save keeps. */
function withComments(text: string): string {
  return text
    .replace(/^\[conversations\]$/m, "# Lines: anything that rewrites who a line is waits for my yes.\n[conversations]")
    .replace(/^\[mcp\.github_heka\]$/m, "# The heka organisation's GitHub. One token, the repositories it may touch under scope.\n[mcp.github_heka]")
    .replace(/^\[mcp\.github_heka\.levels\]$/m, "# Triage. Labels are cheap; closing and reopening wait for my yes.\n[mcp.github_heka.levels]")
    .replace(/^(\[mcp\.github_heka\.targets\."ghost-mock\/infra"\])$/m, "# infra is shared with the platform team. Never close there.\n$1");
}

/** [routes] as the daemon reports it (ADR 0031): the digest, desk voice and the phone. */
export const ROUTES: NonNullable<PolicyView["routes"]> = {
  enabled: ["digest", "desk", "phone"],
  escalate_after: 15,
  quiet_hours: "22:00-07:00",
  batch_secs: 60,
  calls_per_hour: 2,
  calls_per_day: 6,
  carry: "count",
  greeting: "Got a minute?",
  urgency: {},
  available: ["digest", "desk", "desktop"],
};

export default function policy(s: Scenario, servers: ServerFixture[]): PolicyView {
  const path = "~/.config/ghost/policy.toml";
  if (s.data === "empty") {
    // No file yet: the daemon reports empty text.
    return { default: null, skills: {}, targets: {}, path, exists: false, text: "", routes: { ...ROUTES, enabled: ["digest", "phone"], quiet_hours: null } };
  }
  const skills: PolicyView["skills"] = {
    github_heka: { set_labels: "quiet", comment: "announce", close: "confirm", reopen: "confirm" },
    conversations: { set_persona: "confirm", reset: "confirm", delete: "confirm" },
  };
  const targets: PolicyView["targets"] =
    s.data === "busy"
      ? { github_heka: { "ghost-mock/infra": { close: "deny", comment: "confirm" } }, github_work: { "acme/public-app": { close: "deny", comment: "confirm" } } }
      : { github_heka: { "ghost-mock/infra": { close: "deny" } } };
  // Busy: the calendar sets one level for everything it does with a `*` line.
  if (s.data === "busy") skills.calendar = { "*": "confirm" };
  const p = { default: null, skills, targets };
  return { ...p, path, exists: true, text: withComments(renderToml(p, servers)), routes: ROUTES };
}
