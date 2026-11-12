import {
  type GhostClient,
  type GhostEvent,
  isEvent,
  type PendingProposal,
  type Status,
  type Turn,
} from "@ghost/client-ts";
import { createContext, type ReactNode, useContext, useEffect, useMemo, useState } from "react";

import { type Activity, addActivity } from "./activity";
import { createHearLive, type HearLive } from "./hearLive";
import { createVoiceLevel, type VoiceLevel } from "./voiceLevel";

export interface GhostState {
  client: GhostClient;
  status: Status | null;
  connected: boolean;
  turns: Turn[];
  pending: PendingProposal | null;
  /** What the live session did, by name and never in words (ADR 0055); empty outside one. */
  activity: Activity[];
  logs: { level: string; message: string; at: string }[];
  lastEvent: GhostEvent | null;
  /** Bumps whenever a conversation is created, edited, deleted or switched (ADR 0014). */
  conversationsVersion: number;
  /** Bumps whenever the waiting list changes (ADR 0029); re-read GET /waiting. */
  waitingVersion: number;
  /** Bumps whenever an app is connected, disconnected or changes state, or the permissions change (ADR 0034); re-read GET /apps. */
  appsVersion: number;
  /** Ghost's voice loudness while it speaks, outside React state (see voiceLevel.ts). Stable for the provider's life. */
  voice: VoiceLevel;
  /** The Hearing test's recorder and the mic's level while it is open, outside React state (see hearLive.ts). Stable for the provider's life. */
  hear: HearLive;
}

const Ctx = createContext<GhostState | null>(null);

/** Keeps a live mirror of the daemon over the event stream; reconnects on close. */
export function GhostProvider({ client, children }: { client: GhostClient; children: ReactNode }) {
  const [status, setStatus] = useState<Status | null>(null);
  const [connected, setConnected] = useState(false);
  const [turns, setTurns] = useState<Turn[]>([]);
  const [pending, setPending] = useState<PendingProposal | null>(null);
  const [activity, setActivity] = useState<Activity[]>([]);
  const [logs, setLogs] = useState<GhostState["logs"]>([]);
  const [lastEvent, setLastEvent] = useState<GhostEvent | null>(null);
  const [conversationsVersion, setConversationsVersion] = useState(0);
  const [waitingVersion, setWaitingVersion] = useState(0);
  const [appsVersion, setAppsVersion] = useState(0);
  const [voice] = useState(createVoiceLevel);
  const [hear] = useState(createHearLive);

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
          // Twenty a second while Ghost speaks: into the store, never into React state.
          if (m.type === "voice") {
            voice.set(m.level);
            return;
          }
          // The recorder's level, twenty a second while it is open: into its store as well.
          if (m.type === "hear") {
            hear.set(m.hear, m.level, m.speaking);
            return;
          }
          setLastEvent(m);
          switch (m.type) {
            case "status":
              setStatus(m.status);
              setPending(m.status.pending ?? null);
              if (m.status.phase === "passive" || m.status.phase === "starting") {
                setTurns([]);
                setActivity([]);
              }
              // The voice's last 0 can be lost with a dropped stream; the phase is the backstop.
              if (m.status.phase !== "speaking" && voice.get().level > 0) voice.set(0);
              break;
            case "turn":
              setTurns((t) => [...t.slice(-199), m.turn]);
              break;
            case "pending":
              setPending(m.pending ?? null);
              break;
            case "activity":
              setActivity((list) => addActivity(list, m.activity));
              break;
            case "log":
              setLogs((l) => [...l.slice(-499), { level: m.level, message: m.message, at: m.at }]);
              break;
            case "outcome":
              break;
            case "conversation":
              setConversationsVersion((v) => v + 1);
              break;
            case "waiting":
            case "question":
              // A question asked or settled changes what the waiting list shows (ADR 0051).
              setWaitingVersion((v) => v + 1);
              break;
            case "apps":
              setAppsVersion((v) => v + 1);
              break;
          }
        },
        () => {
          setConnected(false);
          voice.set(0);
          hear.reset();
          // Tear this subscription down before the next one, or its listeners keep
          // firing alongside the new ones and every event shows up twice.
          const s = stop;
          stop = null;
          s?.();
          if (!cancelled) timer = setTimeout(connect, 1500);
        },
      );
      // The daemon sends its snapshot when the bridge's socket opens, not when this page
      // (re)loads; ask once so a reloaded page is online before the next event.
      client
        .status()
        .then((m) => {
          if (cancelled) return;
          setConnected(true);
          setStatus(m);
          setPending(m.pending ?? null);
        })
        .catch(() => {});
    };
    connect();
    return () => {
      cancelled = true;
      if (timer) clearTimeout(timer);
      stop?.();
    };
  }, [client, voice, hear]);

  const value = useMemo(
    () => ({ client, status, connected, turns, pending, activity, logs, lastEvent, conversationsVersion, waitingVersion, appsVersion, voice, hear }),
    [client, status, connected, turns, pending, activity, logs, lastEvent, conversationsVersion, waitingVersion, appsVersion, voice, hear],
  );
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function useGhost(): GhostState {
  const v = useContext(Ctx);
  if (!v) throw new Error("useGhost must be used inside <GhostProvider>");
  return v;
}
