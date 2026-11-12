import { describe, expect, it } from "vitest";

import { LIST_SHOWN, humanKey, shapeOf } from "../src/screens/session/result";

const pretty = (v: unknown) => JSON.stringify(v, null, 2);

describe("an opened call in plain words", () => {
  it("a list of items reads as the items by their titles", () => {
    const issues = Array.from({ length: 12 }, (_, i) => ({ number: 200 + i, title: `Login loop ${i}`, state: "open", body: "long" }));
    const s = shapeOf(pretty({ total_count: 40, incomplete_results: false, items: issues }));
    expect(s.kind).toBe("list");
    if (s.kind !== "list") return;
    expect(s.count).toBe(12);
    expect(s.total).toBe(40);
    expect(s.items.length).toBe(LIST_SHOWN);
    expect(s.items[0]).toEqual({ title: "Login loop 0", meta: ["#200", "open"] });
  });

  it("one thing reads as its fields, a message as its words", () => {
    const s = shapeOf(pretty({ number: 151, title: "Close it", state: "closed", user: { login: "dana" }, labels: [{ name: "bug" }, { name: "p1" }], updated_at: "2026-11-12T10:00:00Z" }));
    expect(s.kind).toBe("fields");
    if (s.kind !== "fields") return;
    expect(s.title).toBe("Close it");
    expect(s.fields).toContainEqual(["User", "dana"]);
    expect(s.fields).toContainEqual(["Labels", "bug, p1"]);
    expect(shapeOf(pretty({ text: "Sent." }))).toEqual({ kind: "text", text: "Sent." });
  });

  it("an empty list says so; a preview cut mid-JSON stays raw", () => {
    expect(shapeOf(pretty({ items: [] }))).toEqual({ kind: "list", count: 0, total: null, items: [] });
    expect(shapeOf('{"items": [{"title": "a"}, {"tit').kind).toBe("raw");
    expect(shapeOf("42").kind).toBe("raw");
  });

  it("keys read as words", () => {
    expect(humanKey("updated_at")).toBe("Updated at");
    expect(humanKey("htmlUrl")).toBe("Html url");
  });
});
