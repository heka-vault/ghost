/**
 * Composes every owner's fixture for a scenario. Foundation-owned: page designers edit
 * their own fixture file, not this one. A new prototype owner adds one line below.
 */
import type { Scenario } from "../scenarios";
import apps, { CATALOG } from "./apps";
import audit from "./audit";
import conversations from "./conversations";
import departments from "./departments";
import mobile from "./mobile";
import onboarding from "./onboarding";
import policy from "./policy";
import review from "./review";
import session from "./session";
import settings from "./settings";
import skills from "./skills";
import sources from "./sources";
import tray from "./tray";
import waiting from "./waiting";

export function composeFixtures(s: Scenario) {
  const appList = apps(s);
  const skillList = skills(s, appList);
  return {
    session: session(s),
    /** The agents (GET /conversations), ADR 0024, by department. */
    conversations: conversations(s),
    /** The department folders (GET /departments), ADR 0024, without their agents, which the mock fills in. */
    departments: departments(s),
    /** The apps in apps.json (GET /apps) and the catalog to connect more from (GET /catalog), ADR 0034. */
    apps: appList,
    catalog: structuredClone(CATALOG),
    /** Ghost's own sources (GET /sources); the mock adds one per connected app. */
    sources: sources(s),
    policy: policy(s),
    audit: audit(s),
    settings: settings(s),
    /** The SKILL.md folders Ghost reads (GET /skills), ADR 0023; `missing` follows the connected apps. */
    skills: skillList,
    /** What needs the operator (GET /waiting), ADR 0029. */
    waiting: waiting(s),
    /** Data with no API behind it yet, exposed to prototypes through useFixture(owner). */
    prototypes: {
      onboarding: onboarding(s),
      review: review(s),
      tray: tray(s),
      mobile: mobile(s),
    } as Record<string, unknown>,
  };
}

export type Fixtures = ReturnType<typeof composeFixtures>;
