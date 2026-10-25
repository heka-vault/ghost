/**
 * Owner: departments. The department folders (ADR 0024) as GET /departments reports them:
 * Ghost at the config root and each department under departments/, with its rules, shared
 * memory, servers, skills, docs, agents and problems. Agents come from the conversations
 * fixture; the mock fills `agents` from them.
 */
import type { DepartmentView } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";

export const CONFIG_DIR = "~/.config/ghost";

interface Seed {
  slug: string;
  title: string;
  aliases?: string[];
  voice?: string | null;
  root?: string | null;
  rules: string;
  memory?: string;
  servers?: string[];
  uses?: string[];
  skills?: string[];
  docs?: string[];
  problems?: string[];
}

const GHOST: Seed = {
  slug: "ghost",
  title: "Ghost",
  rules: "# Ghost\n\nShort answers. Say what changed and what needs me, then stop.\nRead issue numbers plain: “app 151”, never the title unless asked.",
  memory: "",
  servers: ["github"],
  skills: ["invoicing", "triage"],
};

const ACCOUNTING: Seed = {
  slug: "accounting",
  title: "Accounting",
  aliases: ["billing", "the books"],
  voice: "bm_george",
  rules: "# Accounting: the books, invoices and billing bugs\n\nNumbers first, no small talk. Amounts in full, with the currency.\nNever post an entry you have not read back.",
  memory: "The client is Northwind; FY ends in March.\nQ3 close finished October 9; Northwind and Halcyon refunds still open.",
  servers: ["books"],
  uses: ["github"],
  skills: ["month-close"],
  docs: ["chart-of-accounts.md", "close-checklist.md"],
};

const INFRA: Seed = {
  slug: "infra",
  title: "Infra",
  aliases: ["ops", "the infra repo"],
  voice: "am_onyx",
  rules: "# Infra: deploys, the ledger export, anything paging\n\nSay what broke, what changed and who is on it. Never close an infra issue without asking.",
  memory: "The nightly deploy moved to 02:00 after the October 6 outage.",
  uses: ["github"],
  skills: [],
};

const APP: Seed = {
  slug: "app",
  title: "App",
  aliases: ["the app repo", "frontend"],
  voice: "af_sky",
  root: "~/projects/app",
  rules: "# App\n\nBugs first, then what is waiting on my sign-off.",
  uses: ["github"],
  skills: ["release-notes", "triage"],
  docs: ["runbook.md"],
};

const LEGAL: Seed = {
  slug: "legal",
  title: "Legal",
  aliases: ["compliance", "counsel"],
  voice: "bf_emma",
  rules: "# Legal: licenses, DPAs, anything that could be a compliance problem\n\nQuote exactly; never guess at what a contract says.",
  memory: "Counsel is Marco. The Northwind DPA renews in January.",
  servers: [],
  uses: ["github"],
  skills: [],
  problems: ["~/.config/ghost/departments/legal/mcp.json: contracts: env TOKEN looks like a literal secret; use ${NAME} and set it in Ghost's environment"],
};

export default function departments(s: Scenario): Omit<DepartmentView, "agents">[] {
  const seeds: Seed[] = s.data === "empty" ? [{ ...GHOST, rules: "", skills: [] }] : s.data === "busy" ? [GHOST, ACCOUNTING, INFRA, APP, LEGAL] : [GHOST, ACCOUNTING, INFRA, APP];
  return seeds.map((d) => ({
    slug: d.slug,
    title: d.title,
    aliases: d.aliases ?? [],
    path: d.slug === "ghost" ? CONFIG_DIR : `${CONFIG_DIR}/departments/${d.slug}`,
    root: d.root ?? null,
    voice: d.voice ?? null,
    is_ghost: d.slug === "ghost",
    rules_lead: d.rules.split("\n").map((l) => l.replace(/^#+\s*/, "").trim()).find(Boolean) ?? "",
    rules: d.rules,
    memory: d.memory ?? "",
    servers: d.servers ?? [],
    uses: d.uses ?? [],
    servers_up: d.slug === "ghost" || d.slug === s.session || (s.session != null && false),
    skills: d.skills ?? [],
    docs: d.docs ?? [],
    problems: d.problems ?? [],
  }));
}
