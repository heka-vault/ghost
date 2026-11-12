/**
 * Each tool call an agent makes, as the daemon keeps it (`{"type": "reach"}`, ADR 0060):
 * the app, the tool's name, items, bytes, milliseconds. Never contents.
 *
 * Out of React state, like the voice level: a research run makes dozens of calls a minute,
 * and only the screens that draw them subscribe. The last 200 are kept for a late
 * subscriber.
 */
import type { ReachCall } from "@ghost/client-ts";

export interface ReachLive {
  /** The calls seen since the window opened, oldest first, at most 200. */
  recent(): readonly ReachCall[];
  push(call: ReachCall): void;
  subscribe(fn: (call: ReachCall) => void): () => void;
}

export const REACH_KEPT = 200;

export function createReachLive(): ReachLive {
  let calls: ReachCall[] = [];
  const listeners = new Set<(call: ReachCall) => void>();
  return {
    recent: () => calls,
    push(call) {
      calls = calls.length >= REACH_KEPT ? [...calls.slice(1 - REACH_KEPT), call] : [...calls, call];
      for (const fn of listeners) fn(call);
    },
    subscribe(fn) {
      listeners.add(fn);
      return () => {
        listeners.delete(fn);
      };
    },
  };
}
