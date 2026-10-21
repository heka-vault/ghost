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
          servers: ["github"],
          pending: null,
        }),
      }),
    );
    const s = await client.status();
    expect(s.phase).toBe("passive");
    expect(s.servers).toEqual(["github"]);
  });

  it("lists sources and servers (ADR 0022)", async () => {
    const client = new GhostClient(
      fakeTransport({
        "GET /sources": () => [
          { name: "conversations", kind: "built-in", description: "Your lines", operations: [], read_tools: [], scope_items: [], scope_kind: null },
          { name: "github", kind: "server", description: "Issues.", operations: [{ name: "close", description: "Close an issue.", risk: "high", autonomy: "confirm", properties: {}, required: [] }], read_tools: [], scope_kind: "repository", scope_items: ["acme/app"] },
        ],
        "GET /mcp": () => [
          {
            name: "github",
            transport: "stdio",
            connected: true,
            error: null,
            tools: [{ name: "issue_read", description: "Read an issue.", offered: true }],
            writes: [{ operation: "close", tool: "issue_write", risk: "high", autonomy: "confirm", asked_by: ["close"], target: ["owner", "repo", "issue_number"], read: "issue_read", offered: true, properties: {}, required: [] }],
            scope: { kind: "repository", arguments: ["owner", "repo"], items: ["acme/app"] },
          },
        ],
      }),
    );
    const sources = await client.sources();
    expect(sources.map((s) => s.kind)).toEqual(["built-in", "server"]);
    expect(sources[1].scope_kind).toBe("repository");
    const servers = await client.mcp();
    expect(servers[0].writes[0].read).toBe("issue_read");
    expect(servers[0].tools[0].offered).toBe(true);
  });

  it("lists, reads, adds and removes skills (ADR 0023)", async () => {
    const posted: string[] = [];
    let removed = "";
    const info = { name: "month-close", description: "Close the books each month.", source: "ghost", path: "/home/op/.config/ghost/skills/month-close", requires: ["mcp:quickbooks"], missing: ["mcp:quickbooks"], files: ["checklist.md"] };
    const client = new GhostClient(
      fakeTransport({
        "GET /skills": () => [info],
        "GET /skills/month-close": () => ({ name: "month-close", body: "# Month close\n\nReconcile first.", files: ["checklist.md"] }),
        "POST /skills": (body) => {
          posted.push(body);
          return info;
        },
        "DELETE /skills/month-close": () => {
          removed = "month-close";
          return { ok: true, detail: null };
        },
      }),
    );
    const skills = await client.skills();
    expect(skills[0].missing).toEqual(["mcp:quickbooks"]);
    expect((await client.skill("month-close")).body).toContain("Reconcile");
    await client.addSkill({ path: "/home/op/Downloads/month-close" });
    await client.addSkill({ path: "/home/op/Downloads/month-close", replace: true });
    expect(posted.map((b) => JSON.parse(b))).toEqual([
      { path: "/home/op/Downloads/month-close", name: null, replace: false },
      { path: "/home/op/Downloads/month-close", name: null, replace: true },
    ]);
    expect((await client.removeSkill("month-close")).ok).toBe(true);
    expect(removed).toBe("month-close");
  });

  it("designs toolkits and gives them to a department (ADR 0023)", async () => {
    const put: Record<string, string> = {};
    let deleted = "";
    const view = {
      name: "finance",
      description: "The books.",
      skills: ["month-close"],
      grants: [{ server: "github_heka", reads: null, writes: ["comment", "set_labels"], items: ["ghost-mock/app"], levels: { set_labels: "announce" } }],
      problems: ["toolkit finance: no skill named month-close"],
    };
    const line = { slug: "accounting", name: "Accounting", aliases: [], voice: "bm_george", speed: 1, persona: "", memory: "", notes: [], created_at: "x", last_active_at: null, active: false, is_ghost: false, turn_count: 0, compactions: 0, dir: "~", scope: {}, settled_at: null, department: null, is_department: true, toolkits: ["finance"] };
    const client = new GhostClient(
      fakeTransport({
        "GET /toolkits": () => [view],
        "PUT /toolkits/finance": (body) => {
          put["finance"] = body;
          return view;
        },
        "DELETE /toolkits/finance": () => {
          deleted = "finance";
          return { ok: true, detail: null };
        },
        "PUT /conversations/accounting/toolkits": (body) => {
          put["accounting"] = body;
          return line;
        },
      }),
    );
    const all = await client.toolkits();
    expect(all[0].grants[0].reads).toBeNull();
    expect(all[0].problems).toHaveLength(1);
    const saved = await client.putToolkit("finance", { description: view.description, skills: view.skills, grants: view.grants });
    expect(saved.grants[0].levels.set_labels).toBe("announce");
    expect(JSON.parse(put.finance).grants[0].items).toEqual(["ghost-mock/app"]);
    expect((await client.deleteToolkit("finance")).ok).toBe(true);
    expect(deleted).toBe("finance");
    const d = await client.setConversationToolkits("accounting", ["finance"]);
    expect(d.is_department).toBe(true);
    expect(d.toolkits).toEqual(["finance"]);
    expect(JSON.parse(put.accounting)).toEqual({ toolkits: ["finance"] });
  });

  it("lists, applies and exports department templates (ADR 0023)", async () => {
    let applied = "";
    const tpl = { name: "accounting", title: "Accounting", description: "The books.", source: "shipped", path: "/usr/share/ghost/templates/accounting", aliases: ["the books"], voice: "bm_george", toolkits: ["finance"], skills: ["month-close"], needs: ["quickbooks"] };
    const line = { slug: "accounting", name: "Accounting", aliases: ["the books"], voice: "bm_george", speed: 1, persona: "", memory: "", notes: [], created_at: "x", last_active_at: null, active: false, is_ghost: false, turn_count: 0, compactions: 0, dir: "~", scope: {}, settled_at: null, department: null, is_department: true, toolkits: ["finance"] };
    const client = new GhostClient(
      fakeTransport({
        "GET /templates": () => [tpl],
        "POST /templates/accounting/apply": (body) => {
          applied = body;
          return line;
        },
        "POST /conversations/accounting/export": () => ({ ...tpl, source: "ghost" }),
      }),
    );
    expect((await client.templates())[0].needs).toEqual(["quickbooks"]);
    expect((await client.applyTemplate("accounting", "Books")).toolkits).toEqual(["finance"]);
    expect(JSON.parse(applied)).toEqual({ name: "Books" });
    await client.applyTemplate("accounting");
    expect(JSON.parse(applied)).toEqual({ name: null });
    expect((await client.exportTemplate("accounting")).source).toBe("ghost");
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
    await expect(client.confirm()).rejects.toBeInstanceOf(ApiError);
  });

  it("distinguishes the initial snapshot from events", () => {
    expect(isEvent({ type: "log", level: "INFO", message: "m", at: "x" })).toBe(true);
    expect(isEvent({ phase: "passive" } as never)).toBe(false);
    const t = httpTransport("http://x/api");
    expect(t.baseUrl).toBe("http://x/api");
  });
});
