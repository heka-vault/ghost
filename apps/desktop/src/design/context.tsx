/**
 * The design-mode context. Null in production and whenever ?mock is absent.
 * Ships in production builds (it is a few lines and holds no data); the fixtures and the
 * mock transport do not: main.tsx imports them dynamically behind import.meta.env.DEV.
 */
import { createContext, type ReactNode, useContext } from "react";

export interface ScenarioInfo {
  name: string;
  label: string;
  description: string;
}

export interface DesignState {
  scenario: ScenarioInfo;
  scenarios: ScenarioInfo[];
  /** Prototype-only data by owner (onboarding, review, tray, mobile). See src/design/mock/fixtures/. */
  fixtures: Record<string, unknown>;
  /** Reload the app in another scenario, keeping the current route. */
  setScenario: (name: string | null) => void;
}

const Ctx = createContext<DesignState | null>(null);

export function DesignProvider({ value, children }: { value: DesignState | null; children: ReactNode }) {
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

/** The mock layer's state, or null when running against the real daemon. */
export function useDesign(): DesignState | null {
  return useContext(Ctx);
}

/**
 * A prototype's own fixture. Import the type only:
 *   import type { ReviewFixture } from "../../design/mock/fixtures/review";
 *   const data = useFixture<ReviewFixture>("review");
 */
export function useFixture<T>(owner: string): T | null {
  const d = useContext(Ctx);
  return (d?.fixtures[owner] as T | undefined) ?? null;
}

/** A URL for a route in a given scenario, e.g. scenarioHref("session", "/conversations/legal"). */
export function scenarioHref(scenario: string | null, path = "/"): string {
  const q = scenario ? `?mock=${encodeURIComponent(scenario)}` : "";
  return `${window.location.pathname}${q}#${path}`;
}
