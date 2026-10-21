/**
 * Owner: skills. The SKILL.md folders Ghost reads (ADR 0023), as GET /skills reports them:
 * Ghost's own under ~/.config/ghost/skills, and a project's .agents/skills read in place from
 * each root in GHOST_SKILL_ROOTS. `missing` is worked out here from the servers the mock has
 * connected, so a skill that needs a server no scenario declares (quickbooks) shows as missing
 * everywhere, and one that needs the calendar is ready only in Busy, where that server exists.
 */
import type { SkillInfo } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";
import type { ServerFixture } from "./mcp";

export interface SkillFixture {
  info: SkillInfo;
  /** The SKILL.md text without its front matter (GET /skills/{name}). */
  body: string;
}

export const GHOST_SKILLS_DIR = "~/.config/ghost/skills";
const HEKA = "~/projects/heka";
const FLEET = "~/projects/fleet";

interface Seed {
  name: string;
  description: string;
  /** "ghost", or the project root the skill is read from. */
  source: string;
  requires?: string[];
  files?: string[];
  body: string;
}

const SEEDS: Record<string, Seed> = {
  invoicing: {
    name: "invoicing",
    description: "Draft, send and chase invoices from the hours in the tracker.",
    source: "ghost",
    files: ["template.md", "reminders.md"],
    body: `# Invoicing

Use this when the operator asks to bill a client, send an invoice, or chase a late one.

## Drafting

1. Read the hours for the client and month from the tracker. Round to the quarter hour.
2. Fill template.md: one line per task, the rate from the client's record, no discounts unless told.
3. Read the draft back in two sentences: the total and the due date. Never read every line aloud.

## Sending

Sending is a change. Propose it; the operator's yes sends it. If the total differs from last month's by more than a third, say so before asking.

## Chasing

A reminder goes out at 7, 14 and 30 days overdue, in the words in reminders.md. After 30 days, stop and tell the operator; they decide what happens next.
`,
  },
  "month-close": {
    name: "month-close",
    description: "Close the books each month, from reconciliation to the report.",
    source: "ghost",
    requires: ["mcp:quickbooks"],
    files: ["checklist.md"],
    body: `# Month close

Run this on the first working day of the month, or when the operator says "close the books".

## Order

Work down checklist.md in order. Each step reads before it writes; a step that fails stops the close, and the operator hears which one.

1. Reconcile every bank and card account against the statements.
2. Post accruals for work done but not yet invoiced.
3. Review open invoices older than 60 days and list them, without chasing.
4. Lock the period.
5. Produce the report: income, expenses, cash, and the three largest changes from last month.

## Reading it back

Two sentences: where cash stands and the one thing that moved most. Offer the full report by email, never aloud.
`,
  },
  triage: {
    name: "triage",
    description: "Sort new heka issues: label, ask for what is missing, hand the clear ones to an agent.",
    source: HEKA,
    requires: ["mcp:github_heka"],
    files: ["labels.md"],
    body: `# Triage

For an issue with needs-triage, decide which of the five labels in labels.md it gets next.

- Reproducible, scoped, and the fix is obvious from the text: ready-for-agent.
- Needs a design decision or touches billing, auth or data: ready-for-human.
- Can't be understood or reproduced from what's there: needs-info, with one comment asking for exactly what's missing.
- Out of scope or a duplicate: wontfix, saying which issue it duplicates.

Never rename a label. Never close an issue from here; propose that separately and say why.
`,
  },
  "release-notes": {
    name: "release-notes",
    description: "Write release notes from the merged pull requests since the last tag.",
    source: HEKA,
    requires: ["mcp:github_heka"],
    body: `# Release notes

List the pull requests merged since the last tag, group them as Added, Changed and Fixed, and write one plain line per change in the operator's voice. Leave out dependency bumps and anything marked chore. Read the Added lines aloud; the rest go in the file.
`,
  },
  "design-review": {
    name: "design-review",
    description: "Compare a screen against real apps on Mobbin before changing it.",
    source: "ghost",
    requires: ["mcp:mobbin"],
    files: ["checklist.md"],
    body: `# Design review

Before redesigning a screen, find three apps on Mobbin that solve the same job and note what they agree on. Then go through checklist.md: one focal point, both themes, keyboard, empty and error states. Report the three agreements and the one thing the screen does differently, and whether that difference is deliberate.
`,
  },
  standup: {
    name: "standup",
    description: "A two-minute morning briefing: today's calendar, what's waiting on you, what's overdue.",
    source: "ghost",
    requires: ["mcp:calendar", "mcp:github_heka"],
    body: `# Standup

When the operator says "morning" or asks for the standup, say in this order, each in one sentence:

1. The first meeting and how long until it.
2. Anything waiting on their okay in Ghost.
3. Issues assigned to them with ready-for-human that are older than three days.

Stop there. Offer the rest only if asked.
`,
  },
  "expense-report": {
    name: "expense-report",
    description: "Turn a month of receipts into the expense report the accountant wants.",
    source: "ghost",
    requires: ["mcp:quickbooks"],
    files: ["categories.md", "report.md"],
    body: `# Expense report

Match each receipt in the month's folder to a category from categories.md, flag any over 200 without a note, and fill report.md. Submitting is a change: propose it with the total and the number of flagged lines.
`,
  },
  "fleet-onboarding": {
    name: "fleet-onboarding",
    description: "Set up a new machine on the fleet: WireGuard peer, vault access, the first deploy.",
    source: FLEET,
    files: ["peer.md", "vault.md"],
    body: `# Fleet onboarding

A new machine joins in three steps, each its own change: a WireGuard peer (peer.md), a vault policy for its service account (vault.md), then one deploy to prove the path. Never skip the deploy; a peer that has never deployed is not on the fleet.
`,
  },
};

/** A seed as GET /skills reports it, with `missing` worked out from the servers connected now. */
export function skillInfo(seed: Seed, connected: Set<string>): SkillInfo {
  const requires = seed.requires ?? [];
  return {
    name: seed.name,
    description: seed.description,
    source: seed.source,
    path: seed.source === "ghost" ? `${GHOST_SKILLS_DIR}/${seed.name}` : `${seed.source}/.agents/skills/${seed.name}`,
    requires,
    missing: missingOf(requires, connected),
    files: [...(seed.files ?? [])],
  };
}

/** The servers a skill needs that aren't connected: "mcp:<name>" entries with no connected server. */
export function missingOf(requires: string[], connected: Set<string>): string[] {
  return requires.filter((r) => r.startsWith("mcp:") && !connected.has(r.slice(4)));
}

const NORMAL = ["invoicing", "month-close", "triage"];
const BUSY = ["invoicing", "month-close", "design-review", "standup", "expense-report", "triage", "release-notes", "fleet-onboarding"];

export default function skills(s: Scenario, servers: ServerFixture[]): SkillFixture[] {
  if (s.data === "empty") return [];
  const connected = new Set(servers.filter((f) => f.info.connected).map((f) => f.info.name));
  const names = s.data === "busy" ? BUSY : NORMAL;
  return names.map((n) => ({ info: skillInfo(SEEDS[n], connected), body: SEEDS[n].body }));
}
