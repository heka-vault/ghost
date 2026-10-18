/**
 * Composes every owner's fixture for a scenario. Foundation-owned: page designers edit
 * their own fixture file, not this one. A new prototype owner adds one line below.
 */
import type { Scenario } from "../scenarios";
import abilities from "./abilities";
import audit from "./audit";
import conversations from "./conversations";
import mobile from "./mobile";
import onboarding from "./onboarding";
import policy from "./policy";
import review from "./review";
import session from "./session";
import settings from "./settings";
import tray from "./tray";

export function composeFixtures(s: Scenario) {
  const ab = abilities(s);
  return {
    session: session(s),
    conversations: conversations(s),
    abilities: ab,
    policy: policy(s, ab),
    audit: audit(s),
    settings: settings(s),
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
