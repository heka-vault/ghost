import { describe, expect, it } from "vitest";

import { ApiError, GhostClient, httpTransport, isEvent, type Transport } from "../src";

function fakeTransport(routes: Record<string, (body: string) => unknown>): Transport {
  // openapi-fetch hands fetch a Request object; read method and body from it.
  const fetchImpl: typeof fetch = async (input, init) => {
    const req = input instanceof Request ? input : new Request(String(input), init);
    const url = new URL(req.url);
    const key = `${req.method.toUpperCase()} ${url.pathname}`;
    const handler = routes[key];
    if (!handler) return new Response(JSON.stringify({ detail: "nope" }), { status: 404 });
    const body = handler(await req.text());
    return new Response(JSON.stringify(body), {
      status: 200,
      headers: { "content-type": "application/json" },
    });
  };
  return { baseUrl: "http://ghost", fetch: fetchImpl, events: { subscribe: () => () => {} } };
}

describe("GhostClient", () => {
  it("types the status response", async () => {
    const client = new GhostClient(
      fakeTransport({
        "GET /status": () => ({
          phase: "passive",
          session_id: null,
          models_ready: true,
          uptime_secs: 1,
          version: "t",
          api_version: "1",
          wake_mode: "file",
          abilities: ["github_issues"],
          pending: null,
        }),
      }),
    );
    const s = await client.status();
    expect(s.phase).toBe("passive");
    expect(s.abilities).toEqual(["github_issues"]);
  });

  it("posts a typed turn and surfaces errors", async () => {
    let sent = "";
    const client = new GhostClient(
      fakeTransport({
        "POST /session/say": (body) => {
          sent = body;
          return { ok: true, detail: null };
        },
      }),
    );
    expect((await client.say("hello")).ok).toBe(true);
    expect(JSON.parse(sent)).toEqual({ text: "hello" });
    await expect(client.undo()).rejects.toBeInstanceOf(ApiError);
  });

  it("distinguishes the initial snapshot from events", () => {
    expect(isEvent({ type: "log", level: "INFO", message: "m", at: "x" })).toBe(true);
    expect(isEvent({ phase: "passive" } as never)).toBe(false);
    const t = httpTransport("http://x/api");
    expect(t.baseUrl).toBe("http://x/api");
  });
});
