/**
 * The site's hero (heyghost.app, docs/site.md) plays the mock's `floating-site` loop on the
 * clock of the site's sound (site/audio/turn.json, rendered into
 * src/design/mock/fixtures/floating-site-turn.json): Ghost arrives as "Hey Ghost" ends,
 * reaches into the README hero's six places before it speaks, speaks each line for as long
 * as its clip lasts, has a yes waiting as it asks, leaves, and comes back, with no long quiet
 * stretch and no place named after anything the operator didn't choose to show.
 */
import type { GhostEvent, Status } from "@ghost/client-ts";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MockDaemon, SITE_TURN } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";

const line = (id: string) => {
  const l = SITE_TURN.lines.find((x) => x.id === id);
  if (!l) throw new Error(id);
  return l;
};

describe("the site's floating loop", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function play(onSiteLoop?: () => void) {
    const s = scenarioByName("floating-site");
    expect(s.name).toBe("floating-site");
    const d = new MockDaemon(s, composeFixtures(s), { onSiteLoop });
    const seen: { at: number; e: GhostEvent }[] = [];
    // The first message is the status snapshot itself; the rest are events.
    const asEvent = (e: GhostEvent | Status): GhostEvent => ("type" in e ? e : { type: "status", status: e });
    const start = Date.now();
    d.subscribe((e) => seen.push({ at: Date.now() - start, e: asEvent(e) }));
    return { seen, d };
  }
  const firstAt = (seen: ReturnType<typeof play>["seen"], test: (e: GhostEvent) => boolean, after = 0) =>
    seen.find((x) => x.at >= after && test(x.e))?.at ?? Infinity;

  it("is a short loop with time to speak", () => {
    // Longer than the first 18.5 s cut so nothing is rushed, short enough to watch through.
    expect(SITE_TURN.loop_ms).toBeGreaterThan(18_000);
    expect(SITE_TURN.loop_ms).toBeLessThan(27_000);
    expect(SITE_TURN.sleep + 1500).toBeLessThan(SITE_TURN.loop_ms);
    // Lines in order, none over another.
    for (const [a, b] of SITE_TURN.lines.slice(0, -1).map((l, i) => [l, SITE_TURN.lines[i + 1]] as const)) expect(a.at + a.ms).toBeLessThanOrEqual(b.at);
    expect(SITE_TURN.voice.levels.length).toBe(Math.ceil(SITE_TURN.loop_ms / SITE_TURN.voice.window_ms));
  });

  it("arrives, reaches, speaks and asks for a yes on the sound's clock", () => {
    const { seen } = play();
    vi.advanceTimersByTime(SITE_TURN.loop_ms - 100);
    const woke = firstAt(seen, (e) => e.type === "status" && !!e.status.session_id);
    const calls = seen.filter((x) => x.e.type === "activity" && x.e.activity.kind === "call");
    const sixth = calls[5]?.at ?? Infinity;
    const replied = firstAt(seen, (e) => e.type === "status" && e.status.phase === "speaking");
    const yes = firstAt(seen, (e) => e.type === "pending" && !!e.pending);
    const ending = firstAt(seen, (e) => e.type === "status" && !!e.status.ending);
    const asleep = firstAt(seen, (e) => e.type === "status" && !e.status.session_id, woke);
    expect(woke).toBe(SITE_TURN.wake);
    expect(woke).toBeLessThan(2000);
    expect(sixth).toBeLessThan(line("reply").at);
    expect(replied).toBe(line("reply").at);
    expect(yes).toBe(line("ask").at);
    expect(ending).toBe(line("bye").at);
    expect(asleep).toBe(SITE_TURN.sleep);
  });

  it("says the words the clips say, for as long as they last", () => {
    const { seen } = play();
    vi.advanceTimersByTime(SITE_TURN.loop_ms - 100);
    const ghost = seen.flatMap((x) => (x.e.type === "turn" && x.e.turn.role === "assistant" && x.e.turn.via === "system" && x.at > SITE_TURN.wake ? [{ at: x.at, text: x.e.turn.text }] : []));
    const said = SITE_TURN.lines.filter((l) => l.who === "ghost").map((l) => ({ at: l.at, text: l.text }));
    expect(ghost).toEqual(said);
    // Each line speaks until its clip ends, then Ghost listens (or the session ends).
    for (const l of SITE_TURN.lines.filter((x) => x.who === "ghost")) {
      const after = firstAt(seen, (e) => e.type === "status" && e.status.phase !== "speaking", l.at + 1);
      expect(after).toBe(l.at + l.ms);
    }
  });

  it("moves Ghost's light with the clip's own level", () => {
    const { seen } = play();
    vi.advanceTimersByTime(SITE_TURN.loop_ms - 100);
    const r = line("reply");
    const levels = seen.filter((x) => x.e.type === "voice" && x.at > r.at && x.at < r.at + r.ms).map((x) => ({ at: x.at, level: (x.e as { level: number }).level }));
    expect(levels.length).toBeGreaterThan(r.ms / 60);
    for (const { at, level } of levels) expect(level).toBe(SITE_TURN.voice.levels[Math.floor(at / SITE_TURN.voice.window_ms)] ?? 0);
    expect(Math.max(...levels.map((x) => x.level))).toBeGreaterThan(0.5);
  });

  it("comes back for another turn soon after it leaves, and says when each starts", () => {
    const loops = vi.fn();
    const { seen, d } = play(loops);
    vi.advanceTimersByTime(SITE_TURN.loop_ms + 3000);
    const sessions = new Set(seen.flatMap((x) => (x.e.type === "status" && x.e.status.session_id ? [x.e.status.session_id] : [])));
    expect(sessions.size).toBeGreaterThanOrEqual(2);
    expect(loops).toHaveBeenCalledTimes(2);
    d.dispose();
    const n = seen.length;
    vi.advanceTimersByTime(SITE_TURN.loop_ms * 2);
    expect(seen.length).toBe(n);
  });

  it("looks only in the README hero's places", () => {
    const { seen } = play();
    vi.advanceTimersByTime(line("reply").at);
    const names = seen.flatMap((x) => (x.e.type === "activity" && x.e.activity.kind === "call" ? [x.e.activity.name ?? ""] : []));
    expect(names.map((n) => n.split("_")[0]).sort()).toEqual(["calendar", "ghost", "ghost", "ghost", "github", "linear"]);
    expect(names.join(" ")).not.toMatch(/acme|mobbin/i);
  });
});
