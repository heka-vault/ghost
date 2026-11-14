/**
 * The site's hero (heyghost.app, docs/site.md) plays the mock's `floating-site` loop: Ghost
 * arrives within two seconds, reaches into the README hero's six places by five, speaks by
 * eight, has a yes waiting by twelve, leaves, and comes back, with no long quiet stretch and
 * no place named after anything the operator didn't choose to show.
 */
import type { GhostEvent, Status } from "@ghost/client-ts";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MockDaemon } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";

describe("the site's floating loop", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  function play() {
    const s = scenarioByName("floating-site");
    expect(s.name).toBe("floating-site");
    const d = new MockDaemon(s, composeFixtures(s), {});
    const seen: { at: number; e: GhostEvent }[] = [];
    // The first message is the status snapshot itself; the rest are events.
    const asEvent = (e: GhostEvent | Status): GhostEvent => ("type" in e ? e : { type: "status", status: e });
    const start = Date.now();
    d.subscribe((e) => seen.push({ at: Date.now() - start, e: asEvent(e) }));
    return seen;
  }
  const firstAt = (seen: ReturnType<typeof play>, test: (e: GhostEvent) => boolean) => seen.find((x) => test(x.e))?.at ?? Infinity;

  it("arrives, reaches, speaks and asks for a yes on the operator's clock", () => {
    const seen = play();
    vi.advanceTimersByTime(19_000);
    const woke = firstAt(seen, (e) => e.type === "status" && !!e.status.session_id);
    const calls = seen.filter((x) => x.e.type === "activity" && x.e.activity.kind === "call");
    const sixth = calls[5]?.at ?? Infinity;
    const replied = firstAt(seen, (e) => e.type === "status" && e.status.phase === "speaking");
    const yes = firstAt(seen, (e) => e.type === "pending" && !!e.pending);
    const ending = firstAt(seen, (e) => e.type === "status" && !!e.status.ending);
    expect(woke).toBeLessThan(2000);
    expect(sixth).toBeLessThan(5000);
    expect(replied).toBeLessThan(8000);
    expect(yes).toBeLessThan(12_000);
    expect(ending).toBeLessThan(17_000);
  });

  it("comes back for another turn soon after it leaves", () => {
    const seen = play();
    vi.advanceTimersByTime(22_000);
    const sessions = new Set(seen.flatMap((x) => (x.e.type === "status" && x.e.status.session_id ? [x.e.status.session_id] : [])));
    expect(sessions.size).toBeGreaterThanOrEqual(2);
  });

  it("looks only in the README hero's places", () => {
    const seen = play();
    vi.advanceTimersByTime(8000);
    const names = seen.flatMap((x) => (x.e.type === "activity" && x.e.activity.kind === "call" ? [x.e.activity.name ?? ""] : []));
    expect(names.map((n) => n.split("_")[0]).sort()).toEqual(["calendar", "ghost", "ghost", "ghost", "github", "linear"]);
    expect(names.join(" ")).not.toMatch(/acme|mobbin/i);
  });
});
