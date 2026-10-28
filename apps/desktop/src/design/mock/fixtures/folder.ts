/**
 * Owner: watching (watches and webhooks) and conversations (rules and outputs). Ghost's folder
 * as the daemon reports it (ADR 0035): AGENTS.md (GET /rules), what Ghost made in out/
 * (GET /outputs), and what it watches (GET /watches): watches.json, hooks.json, where the
 * webhook listener is and what is wrong in those files. Extend freely.
 */
import type { HookInfo, OutputInfo, RulesView, WatchInfo, WatchingView } from "@ghost/client-ts";

import { ago, type Scenario } from "../scenarios";

/** Ghost's folder, written from ~ as the design system does. */
export const CONFIG_DIR = "~/.config/ghost";

const RULES =
  "# Ghost\n\n" +
  "Short answers. Say what changed and what needs me, then stop.\n\n" +
  "Read issue numbers plain: “app 151”, never the title unless asked.\n\n" +
  "Never close an issue in ghost-mock/infra without asking.";

export function rules(s: Scenario): RulesView {
  return { path: `${CONFIG_DIR}/AGENTS.md`, text: s.data === "empty" ? "" : RULES };
}

export function outputs(s: Scenario): OutputInfo[] {
  if (s.data === "empty") return [];
  const file = (name: string, bytes: number, minutes: number): OutputInfo => ({ name, path: `${CONFIG_DIR}/out/${name}`, bytes, modified: ago(minutes) });
  const out = [file("week-in-review.html", 18432, 60), file("q3-refunds.csv", 2310, 60 * 26)];
  if (s.data === "busy") out.push(file("release-4.12-notes.md", 1540, 60 * 24 * 5), file("triage-labels.py", 860, 60 * 24 * 9));
  return out;
}

const CI: WatchInfo = {
  name: "ci",
  app: "github",
  read: "list_workflow_runs",
  every_secs: 60,
  steps: ["check", "jev"],
  response: "tell",
  urgency: "notify",
  last_run: ago(1),
  last_ok: ago(1),
  error: null,
  open: 1,
  seen: 14,
};

const REVIEWS: WatchInfo = {
  name: "reviews",
  app: "github",
  read: "search_pull_requests",
  every_secs: 300,
  steps: ["check"],
  response: "tell",
  urgency: "digest",
  last_run: ago(3),
  last_ok: ago(3),
  error: null,
  open: 0,
  seen: 2,
};

/** Busy: a watch whose app stopped answering. */
const CALENDAR: WatchInfo = {
  name: "standup-moved",
  app: "calendar",
  read: "list_events",
  every_secs: 900,
  steps: ["check", "model"],
  response: "record",
  urgency: "digest",
  last_run: ago(4),
  last_ok: ago(60 * 3),
  error: "the calendar app isn’t connected (sign in again under Apps)",
  open: 0,
  seen: 0,
};

const JOBS: HookInfo = {
  name: "jobs",
  path: "/hooks/jobs",
  verify: "hmac",
  events: ["job_failed", "job_ok"],
  on: true,
  off_reason: null,
  accepted: { item: 3, resolved: 2, ignored: 1 },
  rejected: {},
  last: ago(180),
};

const DEPLOYS: HookInfo = {
  name: "deploys",
  path: "/hooks/deploys",
  verify: "github",
  events: ["workflow_run", "deployment_status"],
  on: true,
  off_reason: null,
  accepted: { item: 1, ignored: 12 },
  rejected: { "bad signature": 2, replayed: 1 },
  last: ago(42),
};

/** Busy: a webhook whose secret isn't in the environment, so it stays off. */
const STRIPE: HookInfo = {
  name: "stripe",
  path: "/hooks/stripe",
  verify: "hmac",
  events: ["refund_failed"],
  on: false,
  off_reason: "its password STRIPE_HOOK_SECRET is not set",
  accepted: {},
  rejected: {},
  last: null,
};

export function watching(s: Scenario): WatchingView {
  if (s.data === "empty") return { watches: [], hooks: [], listening: [], problems: [] };
  if (s.data === "busy") {
    return {
      watches: [CI, REVIEWS, CALENDAR],
      hooks: [JOBS, DEPLOYS, STRIPE],
      listening: ["100.64.0.2:8790"],
      problems: [
        `${CONFIG_DIR}/watches.json: invoices: app "quickbooks" is not connected`,
        `${CONFIG_DIR}/hooks.json: billing: event "charge_failed" needs "when" or "resolve"; the hook stays off`,
      ],
    };
  }
  return { watches: [CI, REVIEWS], hooks: [JOBS, DEPLOYS], listening: ["127.0.0.1:8790"], problems: [] };
}
