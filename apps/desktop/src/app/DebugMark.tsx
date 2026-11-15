/**
 * Debug mode's mark (ADR 0073): while Ghost is logging what it hears, the rail's status card
 * says so, unmistakably, with the time left, and leads to Settings › Developer where it is
 * turned off. The floating view wears its own (floating/Floating.tsx). Nothing here turns it
 * on: the status carries `debug_audio` only while the daemon's switch is on.
 */
import type { Status } from "@ghost/client-ts";
import { Bug } from "lucide-react";
import { useEffect, useState } from "react";

import { debugWords } from "../screens/settings/plain";
import { hrefFor } from "./router";

export function useMinute(on: boolean): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!on) return;
    const t = window.setInterval(() => setNow(Date.now()), 15_000);
    return () => window.clearInterval(t);
  }, [on]);
  return now;
}

/** The rail's mark: a pill under the state, a link to where it is turned off. */
export function RailDebugMark({ status }: { status: Status | null }) {
  const mark = status?.debug_audio ?? null;
  const now = useMinute(!!mark);
  const words = debugWords(mark, now);
  if (!words) return null;
  return (
    <a className="rail__debug" href={hrefFor("/settings/developer/debug")} aria-label={`Debug mode is on, ${words}: Ghost is logging what it hears. Open Settings › Developer to turn it off.`}>
      <Bug size={12} aria-hidden />
      <span className="rail__debug-time">{/^(\d+) min/.exec(words)?.[1] ? `${/^(\d+) min/.exec(words)![1]}m` : words.startsWith("under") ? "<1m" : "on"}</span>
    </a>
  );
}
