/** The stream's views to the list on Home, the connection's state, and a ring's link. */
import type { QuestionView, WaitingItemView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { liveLine, NO_LINK, parseLink, parseRingAnswer, sourceWords, staleLine, waitingRows } from "./waiting";

const at = (m: number) => new Date(Date.UTC(2026, 10, 11, 9, m)).toISOString();

function question(key: string, patch: Partial<QuestionView> = {}): QuestionView {
  return {
    key,
    kind: "question",
    in_run: false,
    asked_by: "Ledger",
    source: "agent:ledger",
    question: "Should the October invoice go out today?",
    detail: null,
    choices: ["Yes", "No"],
    choices_only: false,
    urgency: "notify",
    asked_at: at(10),
    expires_at: at(59),
    state: "open",
    answer: null,
    choice: null,
    answered_via: null,
    answered_at: null,
    change_id: null,
    ...patch,
  };
}

function item(key: string, patch: Partial<WaitingItemView> = {}): WaitingItemView {
  return { key, source: "watch:ci", title: "CI failed on main", state: "new", first_seen: at(5), changed_at: at(5), age_secs: 60, ...patch };
}

describe("waitingRows", () => {
  it("shows items, questions and changes, oldest first, with who and when", () => {
    const q = question("question:a");
    const change = question("change:p1", { kind: "change", asked_by: "Ghost", source: "gate", choices_only: true, change_id: "p1", asked_at: at(1) });
    const rows = waitingRows([item("ci:1"), item("question:a", { question: q, first_seen: at(10) }), item("change:p1", { source: "gate", question: change })], [q, change]);
    expect(rows.map((r) => [r.key, r.kind, r.from])).toEqual([
      ["change:p1", "change", "Ghost"],
      ["ci:1", "item", "Watch ci"],
      ["question:a", "question", "Ledger"],
    ]);
    expect(rows[0].changeId).toBe("p1");
    expect(rows[2].choices).toEqual(["Yes", "No"]);
    expect(rows[2].text).toBe("Should the October invoice go out today?");
  });

  it("leaves out what is snoozed and questions no longer open", () => {
    const answered = question("question:a", { state: "answered" });
    const rows = waitingRows([item("ci:1", { state: "snoozed" }), item("question:a", { question: question("question:a") })], [answered]);
    expect(rows).toEqual([]);
  });

  it("the questions list is newer than the item's copy", () => {
    const rows = waitingRows([item("question:a", { question: question("question:a", { question: "old words" }) })], [question("question:a", { question: "new words" })]);
    expect(rows[0].text).toBe("new words");
  });

  it("an open question the list doesn't carry yet is shown too", () => {
    expect(waitingRows([], [question("question:b")]).map((r) => r.key)).toEqual(["question:b"]);
  });

  it("a change only voice can settle has no proposal id", () => {
    const rows = waitingRows([item("change:x", { source: "gate", title: "Waiting for your yes: close #17" })], []);
    expect(rows[0].kind).toBe("change");
    expect(rows[0].changeId).toBeNull();
  });

  it("marks what is on its way and what the desk refused", () => {
    const rows = waitingRows([item("a"), item("b")], [], ["a"], { b: "Already answered." });
    expect(rows.find((r) => r.key === "a")?.sending).toBe(true);
    expect(rows.find((r) => r.key === "b")?.error).toBe("Already answered.");
  });
});

describe("the live connection", () => {
  it("reads the native state", () => {
    const snap = parseLink(
      JSON.stringify({
        link: "connected",
        address: "10.8.0.4:8765",
        via: "tunnel",
        heardAt: Date.UTC(2026, 10, 11, 9, 0),
        enabled: true,
        bootStart: false,
        items: [item("a")],
        questions: [question("question:a")],
        sending: ["a"],
        errors: { b: "nope", c: 3 },
        ringing: [{ ringId: "r1", agent: "ledger", key: "question:a" }],
        paired: true,
      }),
    )!;
    expect(snap.live.state).toBe("connected");
    expect(snap.live.via).toBe("tunnel");
    expect(snap.live.heardAt).toBe("2026-11-11T09:00:00.000Z");
    expect(snap.live.errors).toEqual({ b: "nope" });
    expect(snap.items.map((i) => i.key)).toEqual(["a"]);
    expect(snap.live.ringing[0].ringId).toBe("r1");
    expect(liveLine(snap.live)).toBe("Connected through your tunnel at 10.8.0.4:8765");
  });

  it("tolerates junk", () => {
    expect(parseLink(null)).toBeNull();
    expect(parseLink("not json")).toBeNull();
    expect(parseLink("{}")!.live.state).toBe("off");
  });

  it("says when the list may be stale", () => {
    const now = Date.UTC(2026, 10, 11, 9, 10);
    expect(staleLine({ ...NO_LINK, state: "connected" }, now)).toBeNull();
    expect(staleLine({ ...NO_LINK, state: "offline", heardAt: new Date(Date.UTC(2026, 10, 11, 9, 5)).toISOString() }, now)).toBe("As of 5 min ago. Can’t reach your desk right now; the phone keeps trying.");
    expect(staleLine({ ...NO_LINK, state: "reconnecting", heardAt: null }, now)).toBe("Reconnecting to your desk…");
    expect(liveLine({ ...NO_LINK, state: "connected", via: "Wi-Fi", address: "192.168.1.20:8765" })).toBe("Connected over your home network at 192.168.1.20:8765");
  });
});

describe("parseRingAnswer", () => {
  it("reads the ring the app was opened for", () => {
    expect(parseRingAnswer({ ring: "5d2f8e0c41aa", agent: "ledger", item: "question:0b00bdd56307" })).toEqual({ ringId: "5d2f8e0c41aa", agent: "ledger", item: "question:0b00bdd56307" });
  });

  it("refuses a link that isn't a ring and falls back to Ghost for an odd agent", () => {
    expect(parseRingAnswer({ ring: "../../etc" })).toBeNull();
    expect(parseRingAnswer({})).toBeNull();
    expect(parseRingAnswer({ ring: "r1", agent: "<script>" })).toEqual({ ringId: "r1", agent: "ghost", item: null });
  });
});

describe("sourceWords", () => {
  it("names where an item came from", () => {
    expect(sourceWords("watch:ci")).toBe("Watch ci");
    expect(sourceWords("hook:github")).toBe("Webhook github");
    expect(sourceWords("agent:heka-workshop")).toBe("Heka Workshop");
    expect(sourceWords("gate")).toBe("A change");
    expect(sourceWords("ghost")).toBe("Ghost");
  });
});
