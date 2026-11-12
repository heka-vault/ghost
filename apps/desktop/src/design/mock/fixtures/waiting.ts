/**
 * Owner: waiting. What the mock's GET /waiting starts with (ADR 0029): a CI failure a
 * watch found, a webhook's item, a change waiting for a yes and Ghost's own trouble.
 */
import type { WaitingItemView } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";

const ago = (minutes: number) => new Date(Date.now() - minutes * 60000).toISOString();
const inMinutes = (minutes: number) => new Date(Date.now() + minutes * 60000).toISOString();

export default function waiting(s: Scenario): WaitingItemView[] {
  if (s.data === "empty") return [];
  const items: WaitingItemView[] = [
    {
      key: "ci:18822931",
      source: "watch:ci",
      title: "CI failed on main: test (ubuntu-latest)",
      link: "https://github.com/heka-vault/ghost/actions/runs/18822931",
      state: "new",
      first_seen: ago(12),
      changed_at: ago(12),
      age_secs: 720,
    },
    {
      key: "change:5f0c9a",
      source: "gate",
      title: "Waiting for your yes: On heka-vault/ghost#17: close the issue.",
      state: "delivered",
      first_seen: ago(40),
      changed_at: ago(40),
      age_secs: 2400,
      delivered_at: ago(25),
      delivered_via: "phone",
      change: {
        id: "5f0c9a",
        source: "github",
        operation: "close",
        target: "heka-vault/ghost#17",
        summary: "On heka-vault/ghost#17: close the issue.",
        rationale: "The operator asked to close it once the fix shipped.",
        expires_at: inMinutes(20),
        expires_in_secs: 1200,
      },
    },
    {
      key: "jobs:backup",
      source: "hook:jobs",
      title: "backup failed",
      state: "acknowledged",
      first_seen: ago(180),
      changed_at: ago(180),
      age_secs: 10800,
      acknowledged_at: ago(170),
    },
    // ADR 0048: an outside tool told the operator something.
    {
      key: "client:claude-code:build",
      source: "client:claude-code",
      title: "From Claude Code: the release build finished, all checks green",
      state: "delivered",
      first_seen: ago(6),
      changed_at: ago(6),
      age_secs: 360,
      delivered_at: ago(6),
      delivered_via: "desktop",
    },
    // ADR 0047: what a scheduled run of Ledger answered.
    {
      key: "agent:accounting:schedule:sch-books",
      source: "agent:accounting",
      title: "Morning books check: two refunds over $500 are still pending",
      state: "new",
      first_seen: ago(48),
      changed_at: ago(48),
      age_secs: 2880,
    },
  ];
  if (s.data === "busy") {
    // ADR 0051: a question asked from a scheduled run; the answer starts one more run.
    items.push({
      key: "question:7d1e",
      source: "agent:accounting",
      title: "Accounting asks: Refund the duplicate charge from the business account?",
      state: "new",
      first_seen: ago(20),
      changed_at: ago(20),
      age_secs: 1200,
      question: {
        key: "question:7d1e",
        kind: "question",
        asked_by: "Accounting",
        source: "agent:accounting",
        question: "Refund the duplicate charge from the business account?",
        detail: "Order 4471 was charged twice on Nov 9.",
        choices: ["Business", "Personal"],
        choices_only: false,
        urgency: "notify",
        asked_at: ago(20),
        expires_at: inMinutes(60 * 23),
        state: "open",
        in_run: true,
      },
    });
    items.push({
      key: "app:linear",
      source: "ghost",
      title: "Linear needs you to sign in again",
      state: "new",
      first_seen: ago(3),
      changed_at: ago(3),
      age_secs: 180,
    });
  }
  return items;
}
