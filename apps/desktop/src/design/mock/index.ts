/**
 * The design-mode mock layer. Loaded only by main.tsx in development when the URL has
 * ?mock=<scenario>; production builds never include this folder.
 *
 *   http://localhost:1430/?mock=session#/lines/accounting
 *   ...&latency=0    answer instantly (default 140ms, to show loading states)
 *   ...&drop=2000    the daemon drops 2s after load (any scenario): reconnecting state
 *   ...&back=6000    and comes back 6s after it dropped
 *   ...&connect=600  the event stream's first open takes 600ms: the Connecting state. Past
 *                    the shell's 900ms grace it turns to Offline, then connects.
 *   ...&phone=off|unpaired|paired   the phone listener off, on with no phone, or a phone paired
 *   ...&phone=pairs    on with no phone; the phone pairs 5s after you ask for a code
 *   ...&phone=expires  on with no phone; codes last 8s, so the expired code can be seen
 *   ...&catalog=slow     the catalog's first copy takes 4s (first run does this by itself)
 *   ...&catalog=problem  the copy is three days old and the refresh failed
 *   ...&catalog=empty    the first copy hasn't arrived, and doesn't for 8s
 *   ...&build=dev|dirty|deploy|release|behind   what the daemon says it is (docs/releasing.md);
 *                    behind is last month's deploy, so this window and Ghost differ
 */
import type { Transport } from "@ghost/client-ts";

import type { DesignState } from "../context";
import { scenarioHref } from "../context";
import { MockDaemon, HttpError } from "./daemon";
import { composeFixtures } from "./fixtures";
import { SCENARIOS, type Scenario, scenarioByName } from "./scenarios";

export { SCENARIOS } from "./scenarios";

export function createMock(name: string): { transport: Transport; design: DesignState } {
  const params = new URLSearchParams(window.location.search);
  const phone = params.get("phone");
  const base = scenarioByName(name);
  const listener: Scenario["phone"] =
    phone === "off" || phone === "unpaired" || phone === "paired" ? phone
    : phone === "pairs" || phone === "expires" ? "unpaired"
    : base.phone;
  const scenario: Scenario = { ...base, phone: listener };
  const fixtures = composeFixtures(scenario);
  const daemon = new MockDaemon(scenario, fixtures, {
    pairsAfterMs: phone === "pairs" ? 5000 : 0,
    codeTtlSecs: phone === "expires" ? 8 : 120,
    catalog: (["slow", "problem", "empty"] as const).find((c) => c === params.get("catalog")) ?? null,
    build: (["dev", "dirty", "deploy", "release", "behind"] as const).find((b) => b === params.get("build")) ?? null,
  });
  const latency = Number(params.get("latency") ?? 140);

  // Dropping: after dropMs the stream closes and requests fail like a stopped daemon.
  const dropMs = Number(params.get("drop") ?? scenario.dropAfterMs ?? 0);
  const backMs = Number(params.get("back") ?? 0);
  // The first stream open waits this long (requests made before it answer as usual), so the
  // shell's Connecting state can be seen and screenshotted. Later reconnects are immediate.
  const connectAt = Date.now() + Number(params.get("connect") ?? 0);
  let down = !scenario.daemon;
  const closers = new Set<() => void>();
  if (scenario.daemon && dropMs > 0) {
    setTimeout(() => {
      down = true;
      for (const c of [...closers]) c();
      closers.clear();
      if (backMs > 0) setTimeout(() => (down = false), backMs);
    }, dropMs);
  }

  const mockFetch: typeof fetch = async (input, init) => {
    const req = input instanceof Request ? input : new Request(String(input), init);
    if (down) {
      await new Promise((r) => setTimeout(r, 30));
      throw new TypeError("Failed to fetch");
    }
    const url = new URL(req.url);
    const text = req.method === "GET" || req.method === "HEAD" ? "" : await req.text();
    const body = text ? JSON.parse(text) : null;
    if (latency) await new Promise((r) => setTimeout(r, latency * (0.6 + Math.random() * 0.8)));
    try {
      const data = await daemon.handle(req.method.toUpperCase(), url.pathname, url.searchParams, body);
      return new Response(JSON.stringify(data ?? null), { status: 200, headers: { "content-type": "application/json" } });
    } catch (e) {
      const status = e instanceof HttpError ? e.status : 500;
      const detail = e instanceof Error ? e.message : String(e);
      return new Response(JSON.stringify({ detail }), { status, headers: { "content-type": "application/json" } });
    }
  };

  const transport: Transport = {
    baseUrl: "http://mock.ghost",
    fetch: mockFetch,
    events: {
      subscribe(onEvent, onClose) {
        if (down) {
          const t = setTimeout(() => onClose?.(), 40);
          return () => clearTimeout(t);
        }
        let unsubscribe: (() => void) | null = null;
        const close = () => {
          unsubscribe?.();
          onClose?.();
        };
        const open = () => {
          if (down) return close();
          unsubscribe = daemon.subscribe(onEvent);
          closers.add(close);
        };
        const wait = connectAt - Date.now();
        const t = wait > 0 ? setTimeout(open, wait) : null;
        if (!t) open();
        return () => {
          if (t) clearTimeout(t);
          closers.delete(close);
          unsubscribe?.();
        };
      },
    },
  };

  const design: DesignState = {
    scenario: { name: scenario.name, label: scenario.label, description: scenario.description },
    scenarios: SCENARIOS.map((s) => ({ name: s.name, label: s.label, description: s.description })),
    fixtures: fixtures.prototypes,
    setScenario: (next) => {
      const path = window.location.hash.replace(/^#/, "") || "/";
      window.location.href = scenarioHref(next, path);
    },
  };
  return { transport, design };
}
