import { describe, expect, it } from "vitest";

import { PLACES, placeOf } from "../src/app/places";

describe("where a waiting item leads", () => {
  it("leads each place to its own route", () => {
    expect(placeOf({ place: "model" })).toEqual({ path: "/settings/model", label: "Open Settings › Model" });
    expect(placeOf({ place: "apps" })?.path).toBe("/apps");
  });
  it("leads nowhere without a place, or with one it doesn't know", () => {
    expect(placeOf({ place: null })).toBeNull();
    expect(placeOf({})).toBeNull();
    expect(placeOf({ place: "https://evil.example" as never })).toBeNull();
    expect(placeOf({ place: "toString" as never })).toBeNull();
  });
  it("only ever leads inside the app", () => {
    for (const p of Object.values(PLACES)) expect(p.path.startsWith("/")).toBe(true);
  });
});
