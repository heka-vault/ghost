import { useGhost } from "@ghost/ui";
import { createContext, useContext, useEffect } from "react";

import { type GhostPhase, StatusDot } from "./Display";
import { phaseOf, phaseText } from "./format";

/**
 * The app-wide connection state, decided once by the shell from the event stream:
 *  - connecting:   the first attempt is in flight (about 900ms; nothing alarming)
 *  - online:       the daemon answers
 *  - reconnecting: it answered earlier in this window and stopped; screens keep their
 *                  last known data under the shell's banner, with writes disabled
 *  - offline:      it has not answered since the window opened (the Offline page)
 */
export type ConnectionState = "connecting" | "online" | "reconnecting" | "offline";

/** What every screen needs to show Ghost's state honestly. Read it with useLive(). */
export interface LiveState {
  connection: ConnectionState;
  /** The voice phase: offline when the daemon isn't answering, starting until models load. */
  phase: GhostPhase;
  /** The daemon answers right now. Anything that writes (Sleep, Confirm, Save) needs it. */
  online: boolean;
  /** Online, but the voice models are still loading: voice-dependent actions wait. */
  loadingModels: boolean;
  /** Online with no Anthropic token and no model endpoint: Ghost can’t answer yet. */
  setupNeeded: boolean;
  /** Where “Finish setup” goes (Settings, or the onboarding prototype in mock mode). */
  setupHref: string;
  /** Used by useSetupPromptHere(); screens never call it. */
  claimSetupPrompt: (delta: number) => void;
}

/** Provided by the app shell. Screens read it through useLive(), never directly. */
export const LiveContext = createContext<LiveState | null>(null);

/**
 * The shell's connection state, the voice phase, and whether setup is needed: the same
 * values the sidebar's status card and the shell's banners use, so a page never contradicts
 * them. Outside the shell (bare prototypes) it falls back to the event stream alone.
 */
export function useLive(): LiveState {
  const ctx = useContext(LiveContext);
  const { status, connected } = useGhost();
  if (ctx) return ctx;
  return {
    connection: connected ? "online" : "offline",
    phase: phaseOf(status, connected),
    online: connected,
    loadingModels: connected && !!status && (!status.models_ready || status.phase === "starting"),
    setupNeeded: false,
    setupHref: "#/settings/general",
    claimSetupPrompt: () => {},
  };
}

/**
 * The dot and words for Ghost's state, as the sidebar's status card says them:
 * “Connecting…” (still, neutral), “Reconnecting” (amber), “Not running” (red), or the
 * voice phase (“Listening”). The one mapping; never hand-roll phaseText() for a page status.
 */
export function liveLabel(connection: ConnectionState, phase: GhostPhase): { dot: GhostPhase; title: string } {
  if (connection === "connecting") return { dot: "passive", title: "Connecting…" };
  if (connection === "reconnecting") return { dot: "starting", title: "Reconnecting" };
  if (connection === "offline") return { dot: "offline", title: "Not running" };
  return { dot: phase, title: phaseText(phase) };
}

/**
 * A page's live status: `<Page status={<LiveStatus />}>`. Same dot, words and tone as the
 * sidebar card. The dot is always there, so the status's leading edge never moves between
 * phases; it holds still while a shell banner above is already moving (models loading,
 * reconnecting), so the screen never shows two moving signals for one fact.
 */
export function LiveStatus() {
  const { connection, phase, loadingModels } = useLive();
  const { dot, title } = liveLabel(connection, phase);
  const still = connection !== "online" || loadingModels;
  return (
    <>
      <StatusDot phase={dot} size="sm" still={still} />
      {title}
    </>
  );
}

/**
 * Call from a screen whose focal point is itself the setup prompt (the session’s start state
 * says “Finish setup to talk to Ghost”). While `active`, the shell hides its “Not set up”
 * banner, so there is one signal per place.
 */
export function useSetupPromptHere(active: boolean) {
  const { claimSetupPrompt } = useLive();
  useEffect(() => {
    if (!active) return;
    claimSetupPrompt(1);
    return () => claimSetupPrompt(-1);
  }, [active, claimSetupPrompt]);
}
