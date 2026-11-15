/**
 * The site's float plays a mock of its own (src/design/mock/site.ts, docs/site.md), a few
 * kilobytes instead of the whole mock daemon. It must show what the product's mock shows: both
 * are played on the same fake clock for two turns and held to the same events at the same times,
 * as the floating view reads them, and to the same answers on the routes it asks.
 */
import type { GhostEvent, Status } from "@ghost/client-ts";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import { MockDaemon, SITE_TURN as PRODUCT_TURN } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";
import { createSiteMock, SITE_TURN, SiteDaemon } from "../src/design/mock/site";

type Seen = { at: number; e: unknown }[];

/** What of each event the floating view (and its provider) reads; ids made ordinal, times dropped. */
function normalise(seen: { at: number; e: GhostEvent | Status }[]): Seen {
  const ids = new Map<string, string>();
  const id = (s: string | null | undefined) => {
    if (!s) return s ?? null;
    if (!ids.has(s)) ids.set(s, `#${ids.size}`);
    return ids.get(s);
  };
  const status = (s: Status) => ({
    phase: s.phase,
    session_id: id(s.session_id),
    conversation: s.conversation ?? null,
    session_origin: s.session_origin ?? null,
    session_typed: s.session_typed,
    ending: s.ending,
    not_listening: s.not_listening,
    debug_audio: s.debug_audio,
    apps: s.apps,
    models_ready: s.models_ready,
    pending: s.pending ? { id: id(s.pending.id), source: s.pending.source, operation: s.pending.operation, target: s.pending.target, summary: s.pending.summary, rationale: s.pending.rationale } : null,
  });
  return seen.flatMap(({ at, e }): Seen => {
    if (!("type" in e)) return [{ at, e: { snapshot: status(e) } }];
    switch (e.type) {
      case "status":
        return [{ at, e: { status: status(e.status) } }];
      case "turn":
        return [{ at, e: { turn: [e.turn.role, e.turn.text, e.turn.via, id(e.turn.session_id)] } }];
      case "activity":
        return [{ at, e: { activity: [e.activity.kind, e.activity.name ?? null, e.activity.id ?? null, id(e.activity.session_id)] } }];
      case "pending":
        return [{ at, e: { pending: e.pending ? [id(e.pending.id), e.pending.summary, e.pending.target] : null } }];
      case "voice":
        return [{ at, e: { voice: e.level } }];
      case "saying":
        return [{ at, e: { saying: [id(e.saying.session_id), e.saying.reply, e.saying.from, e.saying.to] } }];
      case "heard":
        return [{ at, e: { heard: [id(e.heard.session_id), e.heard.text] } }];
      case "outcome":
        return [{ at, e: { outcome: [id(e.outcome.proposal_id), e.outcome.status, e.outcome.summary, e.outcome.target, e.via] } }];
      case "conversation":
        return [{ at, e: { conversation: [e.active ?? null, e.changed ?? null] } }];
      // Runs on their own (ADR 0060) are the desktop's, not the scene's; the floating view never reads them.
      case "reach":
        return [];
      default:
        return [{ at, e: { other: e.type } }];
    }
  });
}

describe("the site's own mock", () => {
  beforeEach(() => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-11-14T12:00:00Z"));
  });
  afterEach(() => vi.useRealTimers());

  function record(subscribe: (l: (e: GhostEvent | Status) => void) => void) {
    const seen: { at: number; e: GhostEvent | Status }[] = [];
    const start = Date.now();
    subscribe((e) => seen.push({ at: Date.now() - start, e }));
    return seen;
  }

  it("plays the product's scene, event for event, on the same clock", async () => {
    const s = scenarioByName("floating-site");
    const productLoops = vi.fn();
    const siteLoops = vi.fn();
    const product = new MockDaemon(s, composeFixtures(s), { onSiteLoop: productLoops });
    const site = new SiteDaemon(siteLoops);
    const a = record((l) => product.subscribe(l));
    const b = record((l) => site.subscribe(l));
    // Two whole turns and into the third.
    for (let t = 0; t < SITE_TURN.loop_ms * 2 + 3000; t += 50) await vi.advanceTimersByTimeAsync(50);
    const pa = normalise(a);
    const pb = normalise(b);
    expect(pb.length).toBeGreaterThan(200);
    expect(pb).toEqual(pa);
    expect(siteLoops).toHaveBeenCalledTimes(3);
    expect(productLoops).toHaveBeenCalledTimes(3);
    product.dispose();
    site.dispose();
  });

  it("is the same clock and the same words", () => {
    expect(SITE_TURN).toBe(PRODUCT_TURN);
  });

  it("answers what the floating view asks as the product's mock does", async () => {
    const s = scenarioByName("floating-site");
    const product = new MockDaemon(s, composeFixtures(s), {});
    const { transport, dispose } = createSiteMock();
    product.subscribe(() => undefined);
    transport.events.subscribe(() => undefined, () => undefined);
    // Into the thinking: every call out and back.
    await vi.advanceTimersByTimeAsync(SITE_TURN.lines.find((l) => l.id === "reply")!.at + 10);
    const get = async (path: string) => {
      const r = await transport.fetch(new Request(`http://mock.ghost${path}`));
      return { status: r.status, body: await r.json() };
    };
    for (const id of ["site0", "site1", "site2", "site3", "site4", "site5"]) {
      const want = (await product.handle("GET", `/calls/${id}`, new URLSearchParams(), null)) as { preview: string; name: string; read: boolean; dropped: number };
      const got = await get(`/calls/${id}`);
      expect(got.status).toBe(200);
      expect([got.body.name, got.body.read, got.body.preview, got.body.dropped]).toEqual([want.name, want.read, want.preview, want.dropped]);
    }
    const turns = (x: { role: string; text: string; via: string }[]) => x.map((t) => [t.role, t.text, t.via]);
    expect(turns((await get("/session/turns")).body)).toEqual(turns((await product.handle("GET", "/session/turns", new URLSearchParams(), null)) as never));
    expect((await get("/questions")).body).toEqual(await product.handle("GET", "/questions", new URLSearchParams(), null));
    expect((await get("/status")).body.phase).toBe("speaking");
    // Nothing else is on the site.
    expect((await get("/audit")).status).toBe(404);
    expect((await get("/calls/nope")).status).toBe(404);
    product.dispose();
    dispose();
  });

  it("stops every timer when disposed", async () => {
    const { transport, dispose } = createSiteMock();
    const seen: unknown[] = [];
    transport.events.subscribe((e) => seen.push(e), () => undefined);
    await vi.advanceTimersByTimeAsync(SITE_TURN.loop_ms / 2);
    dispose();
    const n = seen.length;
    await vi.advanceTimersByTimeAsync(SITE_TURN.loop_ms * 2);
    expect(seen.length).toBe(n);
    expect(vi.getTimerCount()).toBe(0);
  });
});
