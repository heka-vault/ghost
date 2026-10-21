/**
 * Owner: conversations. Department templates (ADR 0023), as GET /templates reports them: the
 * ones Ghost ships and the operator's own under ~/.config/ghost/templates. A template names
 * toolkits and skills; `needs` is what the file is missing for it to work in full (a server
 * one of its toolkits grants that isn't declared, a server one of its skills requires), worked
 * out here from the servers each scenario declares, as the daemon does.
 */
import type { TemplateView } from "@ghost/client-ts";

import type { Scenario } from "../scenarios";
import type { ServerFixture } from "./mcp";
import { TOOLKIT_SEEDS } from "./toolkits";

export interface TemplateFixture {
  view: TemplateView;
  /** The persona the department starts with. */
  persona: string;
  /** What the skills in the template require, for `needs`. */
  requires: string[];
}

const SHIPPED = "/usr/share/ghost/templates";
export const OWN_TEMPLATES_DIR = "~/.config/ghost/templates";

interface Seed {
  view: Omit<TemplateView, "needs">;
  persona: string;
  requires: string[];
}

const SEEDS: Seed[] = [
  {
    view: { name: "triage", title: "Triage", description: "Sort what comes into the heka repositories: label, ask for what is missing, hand the clear ones to an agent.", source: "shipped", path: `${SHIPPED}/triage`, aliases: ["the triage desk", "issues"], voice: "am_michael", toolkits: ["code"], skills: ["triage"] },
    persona: "The triage desk. New issues first, oldest first; say the number, one line on what it is, and which label it gets. Never close from here.",
    requires: ["mcp:github_heka"],
  },
  {
    view: { name: "accounting", title: "Accounting", description: "The books: billing issues, invoices, and the month close, with the finance toolkit and its skills.", source: "shipped", path: `${SHIPPED}/accounting`, aliases: ["the books", "billing"], voice: "bm_george", toolkits: ["finance"], skills: ["month-close", "invoicing"] },
    persona: "You’re the accounting desk. Invoices, vendor renewals and the billing bugs. Numbers first, no small talk.",
    requires: ["mcp:quickbooks"],
  },
];

/** The servers a template still needs: what its toolkits grant and its skills require that the file doesn't declare. */
export function needsOf(toolkits: string[], requires: string[], declared: Set<string>): string[] {
  const granted = toolkits.flatMap((n) => (TOOLKIT_SEEDS[n]?.grants ?? []).map((g) => g.server));
  const required = requires.filter((r) => r.startsWith("mcp:")).map((r) => r.slice(4));
  return [...new Set([...granted, ...required])].filter((s) => !declared.has(s));
}

export default function templates(s: Scenario, servers: ServerFixture[]): TemplateFixture[] {
  const declared = new Set(servers.map((f) => f.info.name));
  const list = SEEDS.map((t) => ({ ...t, view: { ...t.view, needs: needsOf(t.view.toolkits, t.requires, declared) } }));
  // First run: Ghost ships them even before anything is set up, so a department is one step away.
  return s.data === "busy" ? [...list, { view: { name: "security", title: "Security", description: "CVEs, dependabot and anything labeled security, blunt.", source: "ghost", path: `${OWN_TEMPLATES_DIR}/security`, aliases: ["sec"], voice: "am_fenrir", toolkits: ["code"], skills: [], needs: needsOf(["code"], [], declared) }, persona: "CVEs, dependabot, anything labeled security. Be blunt.", requires: [] }] : list;
}
