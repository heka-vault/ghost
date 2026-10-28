/**
 * Owner: skills. The SKILL.md folders Ghost reads (ADR 0023), as GET /skills reports them:
 * Ghost's own under ~/.config/ghost/skills, then the operator's under ~/.agents/skills, read
 * in place (ADR 0035). `requires` names apps (ADR 0034); `missing` is worked out
 * here from the apps the mock has connected, so a skill that needs an app no scenario connects
 * (quickbooks) shows as missing everywhere, and one that needs the calendar is ready only in Busy.
 */
import type { SkillInfo, SkillListing, SkillOriginView } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";
import type { AppFixture } from "./apps";

export interface SkillFixture {
  info: SkillInfo;
  /** The SKILL.md text without its front matter (GET /skills/{name}). */
  body: string;
  /** For one installed from GitHub (#35): the commit its source is on now, when it has moved on. */
  latest?: string;
}

export const GHOST_SKILLS_DIR = "~/.config/ghost/skills";
const HOME_SKILLS_DIR = "~/.agents/skills";

interface Seed {
  name: string;
  description: string;
  /** "ghost" (Ghost's own) or "home" (the operator's ~/.agents/skills). */
  source: string;
  requires?: string[];
  files?: string[];
  body: string;
  /** Installed from GitHub through the catalog (#35), and the commit its source is on now. */
  origin?: SkillOriginView;
  latest?: string;
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
    requires: ["quickbooks"],
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
    source: "home",
    requires: ["github"],
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
    source: "home",
    requires: ["github"],
    body: `# Release notes

List the pull requests merged since the last tag, group them as Added, Changed and Fixed, and write one plain line per change in the operator's voice. Leave out dependency bumps and anything marked chore. Read the Added lines aloud; the rest go in the file.
`,
  },
  "design-review": {
    name: "design-review",
    description: "Compare a screen against real apps on Mobbin before changing it.",
    source: "ghost",
    requires: ["mobbin"],
    files: ["checklist.md"],
    origin: {
      source: "studio-kit/skills",
      source_url: "https://github.com/studio-kit/skills",
      skill_path: "skills/design-review/SKILL.md",
      commit: "4be1c07d2a9f0e63b8a51c27d94e0f1a6c3b8e25",
      folder_hash: "9d3f1a07b6c2e48f5a0d17c3e9b42f6a8d1c05e7",
      ref: null,
      installed_at: "2026-09-30T15:12:04Z",
    },
    latest: "c71e94b0d3a2f85e6b19c0d47a3e2f81b5c6d903",
    body: `# Design review

Before redesigning a screen, find three apps on Mobbin that solve the same job and note what they agree on. Then go through checklist.md: one focal point, both themes, keyboard, empty and error states. Report the three agreements and the one thing the screen does differently, and whether that difference is deliberate.
`,
  },
  standup: {
    name: "standup",
    description: "A two-minute morning briefing: today's calendar, what's waiting on you, what's overdue.",
    source: "ghost",
    requires: ["calendar", "github"],
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
    requires: ["quickbooks"],
    files: ["categories.md", "report.md"],
    origin: {
      source: "ledgerwise/agent-skills",
      source_url: "https://github.com/ledgerwise/agent-skills",
      skill_path: "expense-report/SKILL.md",
      commit: "0a7d5e3c19b84f26e1d0c9a35b7f428e6d1c3b90",
      folder_hash: "e5b0c8d14a7f2963b0e1d5c7a48f3b2e9c6d0a17",
      ref: null,
      installed_at: "2026-10-14T09:40:51Z",
    },
    body: `# Expense report

Match each receipt in the month's folder to a category from categories.md, flag any over 200 without a note, and fill report.md. Submitting is a change: propose it with the total and the number of flagged lines.
`,
  },
  "fleet-onboarding": {
    name: "fleet-onboarding",
    description: "Set up a new machine on the fleet: WireGuard peer, vault access, the first deploy.",
    source: "home",
    files: ["peer.md", "vault.md"],
    body: `# Fleet onboarding

A new machine joins in three steps, each its own change: a WireGuard peer (peer.md), a vault policy for its service account (vault.md), then one deploy to prove the path. Never skip the deploy; a peer that has never deployed is not on the fleet.
`,
  },
};

/** A seed as GET /skills reports it, with `missing` worked out from the apps connected now. */
export function skillInfo(seed: Seed, connected: Set<string>): SkillInfo {
  const requires = seed.requires ?? [];
  return {
    name: seed.name,
    description: seed.description,
    source: seed.source,
    path: seed.source === "ghost" ? `${GHOST_SKILLS_DIR}/${seed.name}` : `${HOME_SKILLS_DIR}/${seed.name}`,
    requires,
    missing: missingOf(requires, connected),
    files: [...(seed.files ?? [])],
    origin: seed.origin ?? null,
  };
}

/** The apps a skill needs that aren't connected. */
export function missingOf(requires: string[], connected: Set<string>): string[] {
  return requires.filter((r) => !connected.has(r));
}

const NORMAL = ["invoicing", "month-close", "triage"];
const BUSY = ["invoicing", "month-close", "design-review", "standup", "expense-report", "triage", "release-notes", "fleet-onboarding"];

export default function skills(s: Scenario, apps: AppFixture[]): SkillFixture[] {
  if (s.data === "empty") return [];
  const connected = new Set(apps.filter((f) => f.view.state === "connected").map((f) => f.view.name));
  const names = s.data === "busy" ? BUSY : NORMAL;
  return names.map((n) => ({ info: skillInfo(SEEDS[n], connected), body: SEEDS[n].body, latest: SEEDS[n].latest }));
}

/* ------------------------------------------------------------------ the catalog (#35) */

/** A skill skills.sh lists, with what GitHub would send for it: its SKILL.md and its files. */
export interface CatalogSkill {
  listing: Omit<SkillListing, "installed">;
  /** The whole SKILL.md, front matter included, as the source wrote it. */
  skillMd: string;
  files: { path: string; bytes: number; script: boolean }[];
  /** Where SKILL.md is in the repository. */
  skillPath: string;
}

const md = (name: string, description: string, body: string) => `---\nname: ${name}\ndescription: ${description}\n---\n\n${body}`;

/** What a search finds, by the words in a name or a repository. Real repositories on skills.sh, invented text. */
export const SKILL_CATALOG: CatalogSkill[] = [
  {
    listing: { id: "anthropics/skills/pdf", source: "anthropics/skills", skill: "pdf", name: "pdf", installs: 218410 },
    skillPath: "skills/pdf/SKILL.md",
    skillMd: md(
      "pdf",
      "Read, fill and merge PDF files: extract text and tables, fill forms, split and combine.",
      `# PDF

Use this when a request involves a PDF: reading one, filling a form, or putting pages together.

## Reading

Extract the text first; fall back to the page images only for a scan. Tables come out as rows, one line each.

## Forms

1. Run scripts/check_fillable_fields.py to see whether the form has fields.
2. If it does, fill them with scripts/fill_fillable_fields.py. If not, read forms.md.
3. Check the result with scripts/create_validation_image.py before handing it back.
`,
    ),
    files: [
      { path: "LICENSE.txt", bytes: 1467, script: false },
      { path: "forms.md", bytes: 11854, script: false },
      { path: "reference.md", bytes: 16692, script: false },
      { path: "scripts/check_fillable_fields.py", bytes: 268, script: true },
      { path: "scripts/create_validation_image.py", bytes: 1258, script: true },
      { path: "scripts/extract_form_structure.py", bytes: 3945, script: true },
      { path: "scripts/fill_fillable_fields.py", bytes: 3819, script: true },
    ],
  },
  {
    listing: { id: "openai/skills/pdf", source: "openai/skills", skill: "pdf", name: "pdf", installs: 13462 },
    skillPath: "skills/pdf/SKILL.md",
    skillMd: md("pdf", "Work with PDF documents.", "# PDF\n\nRead the document page by page and quote page numbers when you answer.\n"),
    files: [],
  },
  {
    listing: { id: "anthropics/skills/docx", source: "anthropics/skills", skill: "docx", name: "docx", installs: 154220 },
    skillPath: "skills/docx/SKILL.md",
    skillMd: md("docx", "Create and edit Word documents, with tracked changes and comments.", "# Word documents\n\nEdit the XML inside the file, never by retyping the document. Keep tracked changes on when the document already has them.\n"),
    files: [
      { path: "ooxml.md", bytes: 23110, script: false },
      { path: "scripts/document.py", bytes: 41022, script: true },
    ],
  },
  {
    listing: { id: "anthropics/skills/xlsx", source: "anthropics/skills", skill: "xlsx", name: "xlsx", installs: 131877 },
    skillPath: "skills/xlsx/SKILL.md",
    skillMd: md("xlsx", "Spreadsheets: formulas, formatting and charts that recalculate.", "# Spreadsheets\n\nWrite formulas, not values. After any change, run scripts/recalc.py and read back the cells you changed.\n"),
    files: [{ path: "scripts/recalc.py", bytes: 6230, script: true }],
  },
  {
    listing: { id: "anthropics/skills/frontend-design", source: "anthropics/skills", skill: "frontend-design", name: "frontend-design", installs: 98310 },
    skillPath: "skills/frontend-design/SKILL.md",
    skillMd: md("frontend-design", "Build interfaces that look designed, not generated.", "# Frontend design\n\nPick one bold direction before writing code: a type pairing, a palette, one signature detail. Then build it fully.\n"),
    files: [],
  },
  {
    listing: { id: "vercel-labs/skills/find-skills", source: "vercel-labs/skills", skill: "find-skills", name: "find-skills", installs: 76021 },
    skillPath: "skills/find-skills/SKILL.md",
    skillMd: md("find-skills", "Find a skill for the task at hand on skills.sh.", "# Find skills\n\nWhen a task would go better with a skill you don't have, search skills.sh and suggest one to the user. Never install one yourself.\n"),
    files: [],
  },
  {
    listing: { id: "github/awesome-copilot/changelog", source: "github/awesome-copilot", skill: "changelog", name: "changelog", installs: 8840 },
    skillPath: "skills/changelog/SKILL.md",
    skillMd: md("changelog", "Keep a CHANGELOG.md in the Keep a Changelog format.", "# Changelog\n\nAdd an entry under Unreleased for every user-facing change. Group as Added, Changed, Fixed, Removed.\n"),
    files: [{ path: "template.md", bytes: 812, script: false }],
  },
  {
    listing: { id: "obra/superpowers/systematic-debugging", source: "obra/superpowers", skill: "systematic-debugging", name: "systematic-debugging", installs: 45210 },
    skillPath: "skills/systematic-debugging/SKILL.md",
    skillMd: md("systematic-debugging", "Find the root cause before proposing a fix.", "# Systematic debugging\n\nReproduce, read the error, form one hypothesis, test it. No fix until the cause is known.\n"),
    files: [{ path: "scripts/find-polluter.sh", bytes: 1652, script: true }],
  },
  {
    listing: { id: "studio-kit/skills/design-review", source: "studio-kit/skills", skill: "design-review", name: "design-review", installs: 2140 },
    skillPath: "skills/design-review/SKILL.md",
    skillMd: md(
      "design-review",
      "Compare a screen against real apps on Mobbin before changing it, then check both themes and the keyboard.",
      `# Design review

Before redesigning a screen, find three apps on Mobbin that solve the same job and note what they agree on.

Then go through checklist.md: one focal point, both themes, keyboard, empty and error states, and now motion: nothing moves that doesn't mean something.

Report the three agreements and the one thing the screen does differently, and whether that difference is deliberate.
`,
    ),
    files: [{ path: "checklist.md", bytes: 1980, script: false }],
  },
];
