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
      key: "engineering:ci:18822931",
      source: "watch:engineering/ci",
      department: "engineering",
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
      department: null,
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
      key: "ghost:job:backup",
      source: "hook:ghost/jobs",
      department: null,
      title: "backup failed",
      state: "acknowledged",
      first_seen: ago(180),
      changed_at: ago(180),
      age_secs: 10800,
      acknowledged_at: ago(170),
    },
  ];
  if (s.data === "busy") {
    items.push({
      key: "app:linear",
      source: "ghost",
      department: null,
      title: "Linear needs you to sign in again",
      state: "new",
      first_seen: ago(3),
      changed_at: ago(3),
      age_secs: 180,
    });
  }
  return items;
}
