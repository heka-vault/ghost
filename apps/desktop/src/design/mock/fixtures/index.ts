/**
 * Composes every owner's fixture for a scenario. Foundation-owned: page designers edit
 * their own fixture file, not this one. A new prototype owner adds one line below.
 */
import type { Scenario } from "../scenarios";
import audit from "./audit";
import conversations from "./conversations";
import mcp from "./mcp";
import mobile from "./mobile";
import onboarding from "./onboarding";
import policy from "./policy";
import review from "./review";
import session from "./session";
import settings from "./settings";
import skills from "./skills";
import sources from "./sources";
import tray from "./tray";

export function composeFixtures(s: Scenario) {
  const servers = mcp(s);
  return {
    session: session(s),
    conversations: conversations(s),
    /** The declared servers (GET /mcp) and what the gate knows of them (GET /sources), ADR 0022. */
    mcp: servers,
    sources: sources(s, servers),
    policy: policy(s, servers),
    audit: audit(s),
    settings: settings(s),
    /** The SKILL.md folders Ghost reads (GET /skills), ADR 0023; `missing` follows the connected servers. */
    skills: skills(s, servers),
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
