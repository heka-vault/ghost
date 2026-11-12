/**
 * Owner: watching (watches and webhooks) and conversations (rules and outputs). Ghost's folder
 * as the daemon reports it (ADR 0035): AGENTS.md (GET /rules), what Ghost made in out/
 * (GET /outputs), and what it watches (GET /watches): watches.json, hooks.json, where the
 * webhook listener is and what is wrong in those files. Extend freely.
 */
import type { HookInfo, OutputInfo, RulesView, ScheduleInfo, ScheduleRunInfo, SchedulesView, WatchInfo, WatchingView } from "@ghost/client-ts";

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
  made_by: null,
  paused: false,
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
  made_by: null,
  paused: false,
};

/** One an agent made (ADR 0052): Ledger watches the refunds it was asked to. */
const REFUNDS: WatchInfo = {
  name: "failed-refunds",
  app: "stripe",
  read: "list_refunds",
  every_secs: 900,
  steps: ["check"],
  response: "tell",
  acts_through: "ledger",
  urgency: "notify",
  last_run: ago(9),
  last_ok: ago(9),
  error: null,
  open: 0,
  seen: 6,
  made_by: "agent:ledger",
  paused: false,
};

/** Busy: an agent's watch that is paused. */
const DEPLOY_WATCH: WatchInfo = {
  name: "slow-deploys",
  app: "github",
  read: "list_deployments",
  every_secs: 600,
  steps: ["check"],
  response: "record",
  urgency: "digest",
  last_run: ago(60 * 5),
  last_ok: ago(60 * 5),
  error: null,
  open: 0,
  seen: 3,
  made_by: "agent:ghost",
  paused: true,
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
  made_by: null,
  paused: false,
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
      watches: [CI, REVIEWS, CALENDAR, REFUNDS, DEPLOY_WATCH],
      hooks: [JOBS, DEPLOYS, STRIPE],
      listening: ["100.64.0.2:8790"],
      problems: [
        `${CONFIG_DIR}/watches.json: invoices: app "quickbooks" is not connected`,
        `${CONFIG_DIR}/hooks.json: billing: event "charge_failed" needs "when" or "resolve"; the hook stays off`,
      ],
    };
  }
  return { watches: [CI, REVIEWS, REFUNDS], hooks: [JOBS, DEPLOYS], listening: ["127.0.0.1:8790"], problems: [] };
}

/** In `minutes` from now, for a schedule's next run. */
const inMinutes = (minutes: number) => new Date(Date.now() + minutes * 60_000).toISOString();

const SCHEDULE_DEFAULTS = {
  at: null,
  every: null,
  cron: null,
  time_zone: "America/Detroit",
  enabled: true,
  created_by: "operator",
  last_error: null,
  running: false,
  active_hours: null,
  grace_minutes: 120,
  max_runtime_minutes: 10,
  max_turns: 30,
  delete_after_run: false,
  skills: [] as string[],
  urgency: "notify" as const,
  notify_failure: true,
  check: null,
};

const MORNING: ScheduleInfo = {
  ...SCHEDULE_DEFAULTS,
  id: "8f2c41d09a3e",
  name: "morning build check",
  agent: "ghost",
  instruction: "Check the build on main in ghost-mock/app. If it is red, say which job failed and who touched it last. If it is green, answer [SILENT].",
  kind: "cron",
  cron: "0 9 * * 1-5",
  summary: "0 9 * * 1-5 (America/Detroit)",
  created_at: ago(60 * 24 * 12),
  updated_at: ago(60 * 24 * 12),
  next_run: inMinutes(60 * 14),
  last_run: ago(60 * 10),
  last_outcome: "ok",
};

const INBOX: ScheduleInfo = {
  ...SCHEDULE_DEFAULTS,
  id: "b71e0c55d2f4",
  name: "vendor inbox",
  agent: "accounting",
  instruction: "Look at what is new in the vendors inbox and tell me about any invoice over a thousand dollars.",
  kind: "every",
  every: "2h",
  summary: "every 2h",
  created_by: "agent:accounting",
  created_at: ago(60 * 24 * 3),
  updated_at: ago(60 * 24 * 3),
  next_run: inMinutes(48),
  last_run: ago(72),
  last_outcome: "unchanged",
  active_hours: { start: "08:00", end: "18:00" },
  urgency: "digest",
  check: { app: "mail", read: "list_unread", args: { folder: "vendors" } },
};

/** Busy: one that could not finish, and one the operator paused. */
const RELEASE: ScheduleInfo = {
  ...SCHEDULE_DEFAULTS,
  id: "0d9a6e3b17c8",
  name: "release notes draft",
  agent: "ghost",
  instruction: "Draft Friday’s release notes from the merged pull requests and save them to out/.",
  kind: "cron",
  cron: "0 15 * * 5",
  summary: "0 15 * * 5 (America/Detroit)",
  created_at: ago(60 * 24 * 30),
  updated_at: ago(60 * 24 * 6),
  next_run: inMinutes(60 * 24 * 2),
  last_run: ago(60 * 24 * 5),
  last_outcome: "failed",
  last_error: "HTTP 529: the model is overloaded",
};

const REMINDER: ScheduleInfo = {
  ...SCHEDULE_DEFAULTS,
  id: "5c3f9b8e21a7",
  name: "quarterly filing",
  agent: "accounting",
  instruction: "Remind me the quarterly filing is due in a week, with the amount from last quarter.",
  kind: "at",
  at: inMinutes(60 * 24 * 20),
  summary: "at 2026-12-01 09:00",
  enabled: false,
  created_at: ago(60 * 24 * 2),
  updated_at: ago(60 * 5),
  next_run: null,
  last_run: null,
  last_outcome: null,
  delete_after_run: true,
};

const run = (schedule: ScheduleInfo, minutes: number, over: Partial<ScheduleRunInfo>): ScheduleRunInfo => ({
  id: `${schedule.id.slice(0, 6)}${minutes}`,
  schedule: schedule.id,
  source: "schedule",
  agent: schedule.agent,
  name: schedule.name,
  status: "ok",
  scheduled_for: ago(minutes),
  started_at: ago(minutes),
  finished_at: ago(minutes - 1),
  answer: null,
  error: null,
  delivery: "none",
  retries: 0,
  model_calls: 2,
  tool_calls: 1,
  ...over,
});

/** What Ghost's agents do on their own (GET /schedules), and what each one's runs said. */
export function schedules(s: Scenario): { view: SchedulesView; runs: Record<string, ScheduleRunInfo[]> } {
  if (s.data === "empty") return { view: { schedules: [], time_zone: "America/Detroit", problems: [] }, runs: {} };
  const runs: Record<string, ScheduleRunInfo[]> = {
    [MORNING.id]: [
      run(MORNING, 60 * 10, { answer: "[SILENT]", delivery: "silent" }),
      run(MORNING, 60 * 34, {
        answer: "The build is red on main: the lint job failed on app#412, which Priya merged at 8:40. The tests passed.",
        delivery: "delivered",
        model_calls: 3,
        tool_calls: 2,
      }),
      run(MORNING, 60 * 58, { answer: "[SILENT]", delivery: "silent" }),
    ],
    [INBOX.id]: [
      run(INBOX, 72, { status: "unchanged", answer: null, model_calls: 0, tool_calls: 0 }),
      run(INBOX, 60 * 26, { answer: "One new invoice over a thousand: Northwind, $2,340, due on the 28th.", delivery: "delivered" }),
    ],
  };
  if (s.data !== "busy") return { view: { schedules: [MORNING, INBOX], time_zone: "America/Detroit", problems: [] }, runs };
  runs[RELEASE.id] = [run(RELEASE, 60 * 24 * 5, { status: "failed", error: "HTTP 529: the model is overloaded", delivery: "failure", retries: 3, model_calls: 4, tool_calls: 0 })];
  return {
    view: {
      schedules: [MORNING, INBOX, RELEASE, REMINDER],
      time_zone: "America/Detroit",
      problems: ["The schedule weekly numbers is for analytics, which is retired; it will not run"],
    },
    runs,
  };
}
