import { type GhostPhase, phaseOf } from "@ghost/ui";
import { createContext, type ReactNode, useContext, useSyncExternalStore } from "react";

import type { Desk, DeskState } from "./types";

const Ctx = createContext<Desk | null>(null);

export function DeskProvider({ desk, children }: { desk: Desk; children: ReactNode }) {
  return <Ctx.Provider value={desk}>{children}</Ctx.Provider>;
}

export interface DeskView extends DeskState {
  desk: Desk;
  /** The phase to show: offline when the link is down, starting until models load. */
  phase: GhostPhase;
  online: boolean;
  loadingModels: boolean;
  /** This phone is in a session. */
  inSession: boolean;
  /** A session is running at the desk itself; the phone can't start one until it ends. */
  deskBusy: boolean;
}

export function useDesk(): DeskView {
  const desk = useContext(Ctx);
  if (!desk) throw new Error("useDesk must be used inside <DeskProvider>");
  const s = useSyncExternalStore(desk.subscribe, desk.getState, desk.getState);
  const online = s.link === "online";
  return {
    ...s,
    desk,
    phase: phaseOf(s.status, online),
    online,
    loadingModels: online && !!s.status && (!s.status.models_ready || s.status.phase === "starting"),
    inSession: !!s.status?.session_id && s.status.session_origin !== "desk",
    deskBusy: !!s.status?.session_id && s.status.session_origin === "desk",
  };
}
