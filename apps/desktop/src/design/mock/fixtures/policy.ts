/** Owner: policy. The autonomy policy file as the daemon reports it. */
import type { AbilityInfo, PolicyView } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";

/** ghost_core.policy.to_toml’s first line. */
const HEADER = "# Ghost autonomy policy (ADR 0011). Edited through the desktop app or by hand.";

/** A TOML key: bare when it can be, quoted otherwise (`"*"`, `"ghost-mock/app"`). */
const key = (k: string) => (/^[A-Za-z0-9_-]+$/.test(k) ? k : JSON.stringify(k));
const list = (xs: string[]) => `[${xs.map((x) => JSON.stringify(x)).join(", ")}]`;

/**
 * The text Ghost writes for a policy, mirroring ghost_core.policy.to_toml: [abilities]
 * (enabled, passive), [defaults], then each ability in name order with its target tables
 * right after it. No comments: a structured save rewrites the file without them.
 */
export function renderToml(p: Pick<PolicyView, "default" | "skills" | "targets" | "enabled"> & { passive?: string[] | null }): string {
  const lines = [HEADER, ""];
  const passive = p.passive ?? [];
  if (p.enabled != null || passive.length) {
    lines.push("[abilities]");
    if (p.enabled != null) lines.push(`enabled = ${list(p.enabled)}`);
    if (passive.length) lines.push(`passive = ${list(passive)}`);
    lines.push("");
  }
  if (p.default) lines.push("[defaults]", `autonomy = "${p.default}"`, "");
  for (const skill of [...new Set([...Object.keys(p.skills), ...Object.keys(p.targets)])].sort()) {
    lines.push(`[${key(skill)}]`);
    for (const [op, lv] of Object.entries(p.skills[skill] ?? {})) lines.push(`${key(op)} = "${lv}"`);
    lines.push("");
    for (const [target, ops] of Object.entries(p.targets[skill] ?? {})) {
      lines.push(`[${key(skill)}.targets."${target}"]`);
      for (const [op, lv] of Object.entries(ops)) lines.push(`${key(op)} = "${lv}"`);
      lines.push("");
    }
  }
  return lines.join("\n").trimEnd() + "\n";
}

/** A file the operator has been into by hand: a few comments that a TOML save keeps. */
function withComments(text: string): string {
  return text
    .replace(/^\[conversations\]$/m, "# Lines: anything that rewrites who a line is waits for my yes.\n[conversations]")
    .replace(/^\[github_issues\]$/m, "# Triage. Labels are cheap; closing and reopening wait for my yes.\n[github_issues]")
    .replace(/^(\[github_issues\.targets\."ghost-mock\/infra"\])$/m, "# infra is shared with the platform team. Never close there.\n$1");
}

export default function policy(s: Scenario, _abilities: AbilityInfo[]): PolicyView {
  const path = "~/.config/ghost/policy.toml";
  if (s.data === "empty") {
    // No file yet: the daemon reports empty text.
    return { default: null, skills: {}, targets: {}, enabled: null, path, exists: false, passive: [], text: "" };
  }
  const skills: PolicyView["skills"] = {
    github_issues: { set_labels: "quiet", comment: "announce", approve: "announce", request_changes: "announce", close: "confirm", reopen: "confirm" },
    conversations: { set_persona: "confirm", reset: "confirm", delete: "confirm" },
  };
  const targets: PolicyView["targets"] =
    s.data === "busy"
      ? { github_issues: { "ghost-mock/infra": { close: "deny", comment: "confirm" } }, release_watch: { "ghost-mock/app": { move_to_milestone: "confirm" } } }
      : { github_issues: { "ghost-mock/infra": { close: "deny" } } };
  // Busy: release_watch sets one level for the whole ability with a `*` line.
  if (s.data === "busy") skills.release_watch = { "*": "confirm" };
  const passive = s.data === "busy" ? ["wake_onnx"] : [];
  const p = { default: null, skills, targets, enabled: null, passive };
  return { ...p, path, exists: true, text: withComments(renderToml(p)) };
}
