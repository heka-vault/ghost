/**
 * How the apps read an audit entry: the gate's summary split into the issue and what happened.
 * Shared by the desktop prototypes and the phone.
 */
import type { AuditEntry } from "@ghost/client-ts";

import { agentName } from "./format";

export type Summarized = Pick<AuditEntry, "summary" | "target"> & { operation?: string };

/**
 * “On ghost-mock/app#208: approved, with …” → { target: "ghost-mock/app#208", text: "Approved, with …" }.
 * A change to an agent (persona, voice) or to Ghost's rules has `conversation: true` and the
 * name as the target: “Standup agent”, “Ghost’s rules”, never a slug in mono.
 */
export function splitSummary(e: Summarized): { target: string; text: string; conversation: boolean } {
  const m = /^On ([^:]+):\s*(.*)$/s.exec(e.summary);
  const text = cap(m ? m[2] : e.summary);
  const agent = e.target.startsWith("agent:") || e.target.startsWith("conversation:");
  const rules = e.target.startsWith("rules:");
  const target = agent ? `${agentName(e.target.slice(e.target.indexOf(":") + 1))} agent` : rules ? "Ghost’s rules" : m ? m[1] : e.target;
  return { target, text, conversation: agent || rules };
}

const cap = (t: string) => t.charAt(0).toUpperCase() + t.slice(1);

/** The change never took effect: nothing on GitHub moved. */
export function neverHappened(e: Pick<AuditEntry, "status">): boolean {
  return e.status === "cancelled" || e.status === "expired" || e.status === "stale" || e.status === "failed" || e.status === "pending";
}

/** An operation in the past tense, for a result: “Closed”, “Approved”, “Labeled”. */
export function pastTense(operation: string): string {
  const known: Record<string, string> = {
    close: "Closed",
    reopen: "Reopened",
    approve: "Approved",
    comment: "Commented",
    set_labels: "Labeled",
    add_labels: "Labeled",
    remove_labels: "Labeled",
    assign: "Assigned",
    unassign: "Unassigned",
    set_milestone: "Milestone set",
    lock: "Locked",
  };
  return known[operation] ?? "Done";
}

/** The short name of an issue for a button label: “ghost-mock/app#208” → “app#208”. */
export function shortTarget(target: string): string {
  return target.split("/").pop() ?? target;
}

/**
 * The change never happened (or no longer stands): canceled, expired, changed first, failed.
 * Its sentence steps down to secondary text so it doesn't read as done. Waiting stays primary.
 */
export function didNotHappen(e: Pick<AuditEntry, "status">): boolean {
  return e.status !== "executed" && e.status !== "pending";
}
