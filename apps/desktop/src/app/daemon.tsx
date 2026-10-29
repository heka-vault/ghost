/**
 * The daemon as the app's own supervisor sees it (ADR 0038): the Offline page says where
 * things stand, and a toast says "Ghost restarted" when the app had to start it again.
 */
import { useEffect, useState } from "react";

import { type DaemonEvent, daemonState, onDaemon } from "../transport";
import { useToast } from "../ui";

/** The supervisor's latest event; null outside Tauri or before it spoke. */
export function useDaemon(): DaemonEvent | null {
  const [event, setEvent] = useState<DaemonEvent | null>(null);
  useEffect(() => {
    let active = true;
    void daemonState().then((e) => {
      if (active && e) setEvent((cur) => cur ?? e);
    });
    const off = onDaemon((e) => {
      if (active) setEvent(e);
    });
    return () => {
      active = false;
      void off.then((f) => f());
    };
  }, []);
  return event;
}

/** What the Offline page says under its title, from the supervisor's state. */
export function daemonLine(e: DaemonEvent | null): string {
  switch (e?.state) {
    case "waiting":
      return "This window is waiting for the daemon make dev runs; make reload starts it.";
    case "restarting":
      return `Ghost stopped (${e.exit}). The app starts it again in ${e.in_secs} s.`;
    case "failed":
      return `Ghost couldn’t start: ${e.error}. The app tries again in ${e.in_secs} s.`;
    case "stopping":
    case "stopped":
      return "Ghost is stopping.";
    case "started":
    case "adopted":
    default:
      return "The app runs Ghost and connects on its own.";
  }
}

/** Mounted once inside the ToastProvider: the restart notice. */
export function DaemonNotices() {
  const toast = useToast();
  useEffect(() => {
    const off = onDaemon((e) => {
      if (e.state === "started" && e.restarts > 0) toast({ title: "Ghost restarted", description: "The app started it again after it stopped.", tone: "info" });
    });
    return () => {
      void off.then((f) => f());
    };
  }, [toast]);
  return null;
}
