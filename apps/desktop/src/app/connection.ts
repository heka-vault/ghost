import { useGhost } from "@ghost/ui";
import { useEffect, useRef, useState } from "react";

import { type ConnectionState, type GhostPhase, phaseOf } from "../ui";

export type { ConnectionState } from "../ui";

const GRACE_MS = 900;

/**
 * The shell's one source for the connection state (see ConnectionState in src/ui/live.tsx).
 * Called once, by App, which provides it to everything below through LiveContext; the
 * sidebar and screens read it with useLive(), so they share one grace timer and never
 * disagree.
 */
export function useConnection(): { state: ConnectionState; phase: GhostPhase; loadingModels: boolean } {
  const { status, connected } = useGhost();
  const ever = useRef(false);
  const [graceOver, setGraceOver] = useState(false);
  if (connected) ever.current = true;

  useEffect(() => {
    const t = setTimeout(() => setGraceOver(true), GRACE_MS);
    return () => clearTimeout(t);
  }, []);

  const state: ConnectionState = connected ? "online" : ever.current ? "reconnecting" : graceOver ? "offline" : "connecting";
  const phase = phaseOf(status, connected);
  const loadingModels = connected && !!status && (!status.models_ready || status.phase === "starting");
  return { state, phase, loadingModels };
}
