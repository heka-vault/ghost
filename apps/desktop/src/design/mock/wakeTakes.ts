/**
 * The mock daemon's kept wake takes (ADR 0064, `ghostd::wake_takes`): a morning's takes,
 * woken and turned down, some labelled; the choice, labels, deleting and playing (playing
 * does nothing here: the daemon plays through Ghost's own speaker). Readings are made up.
 */
import type { WakeTakeLabel, WakeTakeView, WakeTakesView } from "@ghost/client-ts";

import { HttpError } from "./errors";

function curve(peak: number, at: number, n = 170): number[] {
  return Array.from({ length: n }, (_, i) => {
    const d = Math.abs(i - at);
    return Math.max(0.01, peak * Math.exp(-(d * d) / 6) + 0.02 * Math.sin(i));
  });
}

function take(i: number, minutesAgo: number, outcome: "accepted" | "refused", score: number, reading: string, label: WakeTakeLabel | null): WakeTakeView {
  const accepted = outcome === "accepted";
  const secs = accepted ? 13.5 : 5.5;
  const hit = accepted ? 12 : 4;
  return {
    id: `20261113T0${9 + Math.floor(i / 6)}${String(10 + i).padStart(2, "0")}00000-${outcome}`,
    at: new Date(Date.now() - minutesAgo * 60_000).toISOString(),
    outcome,
    model: "hey_ghost",
    score,
    threshold: 0.6,
    input: "ghost.mic",
    array: "alsa_input.usb-XVF3800",
    secs,
    hit_at: hit,
    scores: curve(score, Math.round((hit / 0.08) * 0.95), Math.round(secs / 0.08)),
    scores_end: hit,
    reading,
    label,
    bytes: accepted ? 1_310_000 : 530_000,
  };
}

export class MockWakeTakes {
  private keep = true;
  private unlabelled = false;
  private takes: WakeTakeView[] = [
    take(0, 4, "refused", 0.41, "A ghost.", null),
    take(1, 5, "accepted", 0.82, "Hey Ghost, what needs me?", null),
    take(2, 31, "refused", 0.36, "There it goes.", null),
    take(3, 58, "accepted", 0.64, "A ghost, are you there?", "me"),
    take(4, 95, "refused", 0.33, "He goes to the gym.", "not"),
    take(5, 140, "accepted", 0.71, "Hey Ghost.", "me_missed_before"),
  ];

  view(): WakeTakesView {
    return {
      choice: { keep: this.keep, unlabelled_accepted_are_me: this.unlabelled },
      count: this.takes.length,
      bytes: this.takes.reduce((n, t) => n + t.bytes, 0),
      max_count: 2000,
      max_bytes: 1 << 30,
      playing: null,
      takes: this.takes,
    };
  }

  handle(method: string, path: string, b: Record<string, unknown>): unknown {
    if (!path.startsWith("/wake/takes")) return undefined;
    if (method === "GET" && path === "/wake/takes") return this.view();
    if (method === "DELETE" && path === "/wake/takes") {
      const deleted = this.takes.length;
      this.takes = [];
      return { deleted };
    }
    if (method === "PUT" && path === "/wake/takes/choice") {
      this.keep = !!b.keep;
      this.unlabelled = !!b.unlabelled_accepted_are_me;
      return this.view();
    }
    if (method === "POST" && path === "/wake/takes/play/stop") return { ok: true };
    const m = /^\/wake\/takes\/([^/]+)(\/label|\/play)?$/.exec(path);
    if (!m) return undefined;
    const i = this.takes.findIndex((t) => t.id === m[1]);
    if (i < 0) throw new HttpError(404, "no such take");
    if (method === "PUT" && m[2] === "/label") {
      const label = (b.label ?? null) as WakeTakeLabel | null;
      if (label !== null && !["me", "not", "me_missed_before"].includes(label)) throw new HttpError(422, "label is me, not or me_missed_before");
      this.takes[i] = { ...this.takes[i], label };
      return this.takes[i];
    }
    if (method === "POST" && m[2] === "/play") return { ok: true };
    if (method === "DELETE" && !m[2]) {
      this.takes.splice(i, 1);
      return { ok: true };
    }
    return undefined;
  }
}
