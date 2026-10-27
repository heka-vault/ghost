/**
 * Owner: policy. The policy file as the daemon reports it (ADR 0034): `[permissions]`, what
 * the operator blocks or wants asked first, and `[routes]` (ADR 0031). Everything not named
 * runs; the mode says what a change no rule names does.
 */
import type { PermissionsView, PolicyView } from "@ghost/client-ts";

import { permissionsToml } from "../permissions";
import type { Scenario } from "../scenarios";

export const POLICY_PATH = "~/.config/ghost/policy.toml";

/** ghost_core.policy's first line. */
const HEADER = "# Ghost's policy (ADR 0034): what it may not do, what it asks first, and how it reaches you.";

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

function routesToml(r: NonNullable<PolicyView["routes"]>): string {
  const lines = ["[routes]", `enabled = [${r.enabled.map((e) => JSON.stringify(e)).join(", ")}]`, `escalate_after = ${r.escalate_after}`];
  if (r.quiet_hours) lines.push(`quiet_hours = "${r.quiet_hours}"`);
  lines.push(`greeting = ${JSON.stringify(r.greeting)}`);
  return lines.join("\n");
}

/** The text Ghost writes: its header, `[permissions]`, then `[routes]`. A file the operator has edited keeps its comments in the mock only until a structured save. */
export function renderPolicy(p: PermissionsView, routes: PolicyView["routes"], comments = false): string {
  let perms = permissionsToml(p);
  if (comments) perms = `# Everything runs. These are the exceptions.\n${perms}`;
  return [HEADER, "", perms, "", routes ? routesToml(routes) : ""].join("\n").trimEnd() + "\n";
}

export function permissions(s: Scenario): PermissionsView {
  const base: PermissionsView = { path: POLICY_PATH, mode: "open", block: [], ask: [], quiet: [], allow: [] };
  if (s.data === "empty") return base;
  if (s.data === "busy") {
    return {
      ...base,
      mode: "ask",
      block: ["github.delete_file", "github-acme.*(repo=public-app)", "calendar.delete_event"],
      ask: ["conversations.delete", "conversations.reset"],
      quiet: ["conversations.remember"],
      allow: ["github.add_issue_comment", "github.issue_write(state=open)", "calendar.create_event", "departments.remember"],
    };
  }
  return {
    ...base,
    block: ["github.delete_file", "github.*(owner!=ghost-mock)"],
    ask: ["github.merge_pull_request", "github.issue_write(state=closed)", "conversations.delete", "conversations.reset"],
    quiet: ["conversations.remember"],
  };
}

export default function policy(s: Scenario): PolicyView {
  const perms = permissions(s);
  if (s.data === "empty") {
    // No file yet: the daemon reports empty text and the defaults.
    return { path: POLICY_PATH, exists: false, text: "", permissions: perms, routes: { ...ROUTES, enabled: ["digest", "phone"], quiet_hours: null } };
  }
  return { path: POLICY_PATH, exists: true, text: renderPolicy(perms, ROUTES, true), permissions: perms, routes: ROUTES };
}
