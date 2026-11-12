/**
 * The shell's posture, from `postureOf` (posture.ts) fed with this window's input, Ghost's
 * session and the floating room's state (ADR 0059). Provided once by the shell; `usePosture()`
 * reads it anywhere: the conversation screen takes its room view from here, so the rail's
 * button, Ctrl+., the stage's button and the timer are one switch.
 */
import { useGhost } from "@ghost/ui";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";

import { canFloat, useFloating, useWindowActive } from "./floating";
import { type Posture, type PostureHold, type PostureReason, isRoomKey, nextChangeIn, postureOf, readAway, readSessionRoom, SESSION_IDLE_MS } from "./posture";

export interface PostureState {
  posture: Posture;
  reason: PostureReason;
  /** A live session whose audio is at this desk. */
  deskSession: boolean;
  /** The room is held (Ctrl+., the rail's or the stage's button), not come by itself. */
  held: boolean;
  /** Hold a posture (`room` from the rail's button and Ctrl+.), or `auto` to follow the rule. */
  hold: (h: PostureHold) => void;
  /** Re-read the operator's choices (Settings › General changed them). */
  refresh: () => void;
}

const Ctx = createContext<PostureState>({ posture: "desk", reason: "desk", deskSession: false, held: false, hold: () => {}, refresh: () => {} });

const INPUT = ["pointermove", "pointerdown", "keydown", "wheel", "touchstart"] as const;

export function PostureProvider({ children, enabled = true }: { children: ReactNode; enabled?: boolean }) {
  const { status } = useGhost();
  const deskSession = !!status?.session_id && status.session_origin !== "phone";
  // `?posture=room` opens in the room (screenshots, the gallery); `?posture=desk` never leaves the desk.
  const [hold, setHold] = useState<PostureHold>(() => {
    const p = new URLSearchParams(window.location.search).get("posture");
    return p === "room" || p === "desk" ? p : "auto";
  });
  const [prefs, setPrefs] = useState(() => ({ away: readAway(), session: readSessionRoom() }));
  const last = useRef(Date.now());
  const [, force] = useState(0);
  const holdRef = useRef(hold);
  holdRef.current = hold;
  const floating = useFloating().status;
  const windowActive = useWindowActive();

  const input = () => ({
    hold: enabled ? hold : ("desk" as const),
    deskSession,
    idleMs: Date.now() - last.current,
    awayAfterMs: prefs.away == null ? null : prefs.away * 60_000,
    sessionIdleMs: prefs.session ? SESSION_IDLE_MS : null,
    floating: canFloat(floating),
    windowActive,
  });
  const { posture, reason } = postureOf(input());

  // Input in this window: note the time, and if the room came by itself, come back at once.
  // A small pointer jitter (under 4 px) is not someone at the desk.
  useEffect(() => {
    let px = -1;
    let py = -1;
    const onInput = (e: Event) => {
      if (e.type === "pointermove") {
        const p = e as PointerEvent;
        if (px >= 0 && Math.abs(p.clientX - px) + Math.abs(p.clientY - py) < 4) return;
        px = p.clientX;
        py = p.clientY;
      }
      const wasIdle = Date.now() - last.current;
      last.current = Date.now();
      if (holdRef.current === "auto" && wasIdle > 1000) force((n) => n + 1);
    };
    for (const e of INPUT) window.addEventListener(e, onInput, { passive: true, capture: true });
    return () => {
      for (const e of INPUT) window.removeEventListener(e, onInput, { capture: true });
    };
  }, []);

  // Wake when the rule would next change its mind.
  const wait = nextChangeIn(input());
  useEffect(() => {
    if (wait == null) return;
    const t = window.setTimeout(() => force((n) => n + 1), wait + 50);
    return () => window.clearTimeout(t);
  });

  // Ctrl+. holds the room or lets it go; Escape leaves a held room.
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      if (e.defaultPrevented || e.repeat) return;
      if (isRoomKey(e)) {
        e.preventDefault();
        setHold((h) => (h === "room" ? "auto" : "room"));
        return;
      }
      if (e.key === "Escape" && holdRef.current === "room" && !document.querySelector("dialog[open]")) {
        e.preventDefault();
        setHold("auto");
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  // A session ending lets a held room go back to the rule (the room may stay, as stepped away).
  const hadSession = useRef(deskSession);
  useEffect(() => {
    if (hadSession.current && !deskSession && hold === "room") setHold("auto");
    hadSession.current = deskSession;
  }, [deskSession, hold]);

  const holdFn = useCallback((h: PostureHold) => {
    last.current = Date.now();
    setHold(h);
  }, []);
  const refresh = useCallback(() => setPrefs({ away: readAway(), session: readSessionRoom() }), []);

  // Mark the document, so CSS and the screenshot tool can see the posture.
  useEffect(() => {
    document.documentElement.dataset.posture = posture;
    return () => {
      delete document.documentElement.dataset.posture;
    };
  }, [posture]);

  const held = enabled && hold === "room";
  const value = useMemo(() => ({ posture, reason, deskSession, held, hold: holdFn, refresh }), [posture, reason, deskSession, held, holdFn, refresh]);
  return <Ctx.Provider value={value}>{children}</Ctx.Provider>;
}

export function usePosture(): PostureState {
  return useContext(Ctx);
}

/** The same, under the name the rail and the room use for their buttons. */
export const usePostureControl = usePosture;
