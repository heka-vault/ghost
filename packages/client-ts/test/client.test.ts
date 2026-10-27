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

  it("retires and rehires an agent, and reads Ghost's folder (ADR 0035)", async () => {
    const posts: Record<string, string> = {};
    const watch = { name: "ci", app: "github", read: "list_workflow_runs", every_secs: 60, steps: ["check"], response: "tell", urgency: "notify", last_run: null, last_ok: null, error: null, seen: 0, open: 0 };
    const hook = { name: "ci", path: "/hooks/ci", verify: "github", events: ["workflow_run"], on: true, off_reason: null, accepted: {}, rejected: {}, last: null };
    const client = new GhostClient(
      fakeTransport({
        "POST /conversations/brand/retire": () => ({ ok: true, detail: null }),
        "POST /conversations/brand/rehire": () => ({ ok: true, detail: "not retired" }),
        "GET /conversations": () => [],
        "GET /rules": () => ({ path: "~/.config/ghost/AGENTS.md", text: "# Ghost\n" }),
        "POST /rules": (body) => {
          posts.rule = body;
          return { proposal_id: "p1", target: "Ghost's rules", status: "success", applied: ["add the rule: UK dates."], failed_step: null, error: null, state_now: "3 line(s) of rules", summary: "Applied." };
        },
        "GET /outputs": () => [{ name: "week.html", path: "~/.config/ghost/out/week.html", bytes: 13, modified: "x" }],
        "GET /watches": () => ({ watches: [watch], hooks: [hook], listening: ["127.0.0.1:8766"], problems: [] }),
      }),
    );
    expect((await client.retireConversation("brand")).ok).toBe(true);
    expect((await client.rehireConversation("brand")).detail).toBe("not retired");
    expect(await client.conversations(false)).toEqual([]);
    expect((await client.rules()).text).toBe("# Ghost\n");
    expect((await client.addRule("UK dates.")).status).toBe("success");
    expect(JSON.parse(posts.rule)).toEqual({ text: "UK dates." });
    expect((await client.outputs())[0].name).toBe("week.html");
    const watching = await client.watches();
    expect(watching.watches[0].app).toBe("github");
    expect(watching.hooks?.[0].path).toBe("/hooks/ci");
    expect(watching.listening).toEqual(["127.0.0.1:8766"]);
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
