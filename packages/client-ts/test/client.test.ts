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
          apps: ["github"],
          pending: null,
        }),
      }),
    );
    const s = await client.status();
    expect(s.phase).toBe("passive");
    expect(s.apps).toEqual(["github"]);
  });

  it("lists apps, browses the catalog, connects and moves a permission (ADR 0034)", async () => {
    const sent: Record<string, string> = {};
    const app = {
      name: "github",
      title: "GitHub",
      kind: "local",
      state: "connected",
      actions: [{ name: "issue_write", description: "Change an issue.", reads: false, level: "allow", rule: "github.issue_write", conditional: false }],
      settings: ["GHOST_APP_GITHUB_TOKEN"],
    };
    const perms = { path: "/p", mode: "open", block: [], ask: [], quiet: [], allow: [] };
    const client = new GhostClient(
      fakeTransport({
        "GET /apps": () => [app],
        "GET /catalog": () => ({ entries: [{ name: "app.linear/linear", title: "Linear", description: "Issues.", version: "1", featured: true, options: [{ id: "remote-0", kind: "sign_in", label: "Sign in", settings: [] }], suggested_name: "linear", connected: [] }], total: 1, refreshing: false }),
        "POST /apps": (body) => {
          sent.connect = body;
          return { apps: [{ ...app, name: "linear", title: "Linear", kind: "sign_in", state: "connecting" }], sign_in_url: "https://linear.app/oauth" };
        },
        "PUT /permissions": (body) => {
          sent.permissions = body;
          return { ...perms, ask: ["github.issue_write"] };
        },
      }),
    );
    expect((await client.apps())[0].actions[0].level).toBe("allow");
    const page = await client.catalog("linear");
    expect(page.entries[0].options[0].kind).toBe("sign_in");
    const res = await client.connectApp({ catalog: "app.linear/linear", option: "remote-0", values: {} });
    expect(res.sign_in_url).toContain("linear.app");
    expect(JSON.parse(sent.connect).option).toBe("remote-0");
    const after = await client.updatePermissions({ set: { rule: "github.issue_write", level: "ask" } });
    expect(after.ask).toEqual(["github.issue_write"]);
    expect(JSON.parse(sent.permissions).set.level).toBe("ask");
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

  it("retires, rehires and moves an agent, and reads the departments (ADR 0024)", async () => {
    const posts: Record<string, string> = {};
    const agent = { id: "a1", slug: "brand", name: "Brand", aliases: [], department: "marketing", job: "the look", voice: "bm_george", speed: 1, own_voice: false, persona: "", memory: "", notes: [], created_at: "x", last_active_at: null, retired_at: null, active: false, is_ghost: false, is_default: false, turn_count: 0, compactions: 0, dir: "~" };
    const dept = { slug: "marketing", title: "Marketing", aliases: [], path: "~/.config/ghost/departments/marketing", root: null, voice: null, is_ghost: false, rules_lead: "Marketing", rules: "# Marketing\n", memory: "", permissions: { block: ["github.delete_*"], ask: [], quiet: [] }, skills: [], docs: [], agents: ["marketing", "brand"], problems: [] };
    const client = new GhostClient(
      fakeTransport({
        "POST /conversations/brand/retire": () => ({ ok: true, detail: null }),
        "POST /conversations/brand/rehire": () => ({ ok: true, detail: "not retired" }),
        "POST /conversations/brand/move": (body) => {
          posts.move = body;
          return { ...agent, department: "sales" };
        },
        "GET /departments": () => [dept],
        "GET /departments/marketing": () => dept,
        "POST /departments": (body) => {
          posts.department = body;
          return dept;
        },
        "POST /departments/marketing/rules": (body) => {
          posts.rule = body;
          return { ...dept, rules: "# Marketing\n\nUK dates.\n" };
        },
        "GET /departments/marketing/journal": () => [{ at: "x", agent: "Brand", operation: "github.comment", target: "acme/web#7", what: "On acme/web#7: comment." }],
      }),
    );
    expect((await client.retireConversation("brand")).ok).toBe(true);
    expect((await client.rehireConversation("brand")).detail).toBe("not retired");
    expect((await client.moveConversation("brand", "sales")).department).toBe("sales");
    expect(JSON.parse(posts.move)).toEqual({ department: "sales" });
    expect((await client.departments())[0].permissions?.block).toEqual(["github.delete_*"]);
    expect((await client.department("marketing")).agents).toEqual(["marketing", "brand"]);
    await client.createDepartment({ name: "Marketing", aliases: ["the marketers"], voice: null });
    expect(JSON.parse(posts.department).aliases).toEqual(["the marketers"]);
    expect((await client.addRule("marketing", "UK dates.")).rules).toContain("UK dates");
    expect(JSON.parse(posts.rule)).toEqual({ text: "UK dates." });
    expect((await client.departmentJournal("marketing"))[0].agent).toBe("Brand");
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
