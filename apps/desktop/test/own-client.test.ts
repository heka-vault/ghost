/**
 * The person's own client for signing in (ADR 0076): a sign-in that lets in only registered apps
 * says so and asks for a client ID, never as an error's kind; the secret goes in and never comes
 * back out.
 */
import type { AppConnectResult, AppView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { MockDaemon } from "../src/design/mock/daemon";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";
import { connectOutcome } from "../src/screens/apps/ConnectDialog";
import { stateWords } from "../src/screens/apps/model";
import { ownClientBody } from "../src/screens/apps/OwnClient";

const view = (state: AppView["state"]): AppView => ({ name: "gmail", title: "Gmail", kind: "sign_in", state, problem: null, catalog: null, icon: null, actions: [], settings: [], limits: { timeout: null, connect_timeout: null, result_chars: null, pinned: [], default_timeout: 300, default_connect_timeout: 60, default_result_chars: 24000 } });

describe("an app that needs a client ID of the person's own", () => {
  it("is neither a failure nor still connecting", () => {
    const out = connectOutcome([view("needs_client")]);
    expect(out.failed).toBeNull();
    expect(out.connecting).toBe(false);
    expect(out.needsClient?.name).toBe("gmail");
    expect(stateWords(view("needs_client")).label).toBe("Needs a client ID of yours");
  });

  it("asks for one, signs in with it, and never gives the secret back", async () => {
    const s = scenarioByName("passive")!;
    const d = new MockDaemon(s, composeFixtures(s), {});
    const q = new URLSearchParams();
    await d.handle("GET", "/catalog", q, null);
    const r = (await d.handle("POST", "/apps", q, { catalog: "ghost/gmail", option: "remote" })) as AppConnectResult;
    const gmail = r.apps[0];
    expect(gmail.state).toBe("needs_client");
    expect(gmail.problem).toMatch(/^Google only lets in apps registered with it/);
    expect(gmail.problem).not.toMatch(/RegistrationFailed|MCP|server/i);
    // no address in it, as the plain-words guard holds for everyday pages
    expect(gmail.problem).not.toMatch(/\b[a-z0-9-]+\.[a-z0-9-]+\.(com|app|io|dev|net|org)\b|https?:/);
    expect(gmail.sign_in_client).toMatchObject({ service: "Google", secret: "required", client_id: null });
    const secret = "GOCSPX-a-secret-of-the-persons";
    const again = (await d.handle("PUT", "/apps/gmail/sign-in-client", q, { client_id: "123.apps.googleusercontent.com", client_secret: secret })) as AppConnectResult;
    expect(again.sign_in_url).toBeTruthy();
    expect(again.apps[0].sign_in_client).toMatchObject({ client_id: "123.apps.googleusercontent.com", has_secret: true });
    expect(JSON.stringify(again)).not.toContain(secret);
    expect(JSON.stringify(await d.handle("GET", "/apps", q, null))).not.toContain(secret);
    // an empty secret field keeps the stored secret; only forget_secret takes it away
    const kept = (await d.handle("PUT", "/apps/gmail/sign-in-client", q, { client_id: "456.apps.googleusercontent.com" })) as AppConnectResult;
    expect(kept.apps[0].sign_in_client).toMatchObject({ client_id: "456.apps.googleusercontent.com", has_secret: true });
    const forgot = (await d.handle("PUT", "/apps/gmail/sign-in-client", q, { client_id: "456.apps.googleusercontent.com", forget_secret: true })) as AppConnectResult;
    expect(forgot.apps[0].sign_in_client?.has_secret).toBe(false);
  });
});

describe("the client fields' body", () => {
  it("keeps the stored secret unless the switch says to remove it", () => {
    expect(ownClientBody({ client_id: " abc ", client_secret: "", forget_secret: false })).toEqual({ client_id: "abc", client_secret: null });
    expect(ownClientBody({ client_id: "abc", client_secret: " new ", forget_secret: false })).toEqual({ client_id: "abc", client_secret: "new" });
    // the switch wins over a typed secret: one or the other, never both
    expect(ownClientBody({ client_id: "abc", client_secret: "typed", forget_secret: true })).toEqual({ client_id: "abc", client_secret: null, forget_secret: true });
  });
});

describe("an app from Ghost's own list", () => {
  it("says the registry doesn't list it, rather than naming a publisher it never checked", async () => {
    const { makerWhy } = await import("../src/screens/apps/provenance");
    const s = scenarioByName("passive")!;
    const d = new MockDaemon(s, composeFixtures(s), {});
    const gmail = (await d.handle("GET", "/catalog/ghost%2Fgmail", new URLSearchParams(), null)) as import("@ghost/client-ts").CatalogEntryView;
    expect(gmail.listed_by_ghost).toBe(true);
    expect(gmail.publisher ?? null).toBeNull();
    expect(makerWhy(gmail)).toMatch(/^The public app registry doesn’t list it; Ghost lists it itself/);
  });
});
