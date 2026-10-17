import {
  type GhostClient,
  type GhostEvent,
  isEvent,
  type PendingProposal,
  type Status,
  type Turn,
} from "@ghost/client-ts";
import { createContext, type ReactNode, useContext, useEffect, useMemo, useState } from "react";

export interface GhostState {
  client: GhostClient;
  status: Status | null;
  connected: boolean;
  turns: Turn[];
  pending: PendingProposal | null;
  logs: { level: string; message: string; at: string }[];
  lastEvent: GhostEvent | null;
}

const Ctx = createContext<GhostState | null>(null);

/** Keeps a live mirror of the daemon over the event stream; reconnects on close. */
export function GhostProvider({ client, children }: { client: GhostClient; children: ReactNode }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [connected, setConnected] = useState(false);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [pending, setPending] = useState<PendingProposal | null>(null);
  const [logs, setLogs] = useState<GhostState["logs"]>([]);
  const [lastEvent, setLastEvent] = useState<GhostEvent | null>(null);

  useEffect(() => {
    let stop: (() => void) | null = null;
    let timer: ReturnType<typeof setTimeout> | null = null;
    let cancelled = false;

    const connect = () => {
      stop = client.events.subscribe(
        (m) => {
          setConnected(true);
          if (!isEvent(m)) {
            setStatus(m);
            setPending(m.pending ?? null);
            client.turns().then(setTurns).catch(() => {});
            return;
          }
          setLastEvent(m);
          switch (m.type) {
            case "status":
              setStatus(m.status);
              setPending(m.status.pending ?? null);
              if (m.status.phase === "passive" || m.status.phase === "starting") setTurns([]);
              break;
            case "turn":
              setTurns((t) => [...t.slice(-199), m.turn]);
              break;
            case "pending":
              setPending(m.pending ?? null);
              break;
            case "log":
              setLogs((l) => [...l.slice(-499), { level: m.level, message: m.message, at: m.at }]);
              break;
            case "outcome":
              break;
          }
        },
        () => {
          setConnected(false);
          if (!cancelled) timer = setTimeout(connect, 1500);
        },
      );
    };
    connect();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      stop?.();
    };
  }, [client]);

  const value = useMemo(
    () => ({ client, status, connected, turns, pending, logs, lastEvent }),
    [client, status, connected, turns, pending, logs, lastEvent],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useGhost(): GhostState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useGhost must be used inside <GhostProvider>");
  return v;
}
