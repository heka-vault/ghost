import { describe, expect, it } from "vitest";

import { PLACES, placeOf } from "../src/app/places";
import { resolve } from "../src/app/routes";

describe("where a waiting item leads", () => {
  it("leads each of Ghost's own notes to the section that fixes it", () => {
    for (const [key, place, section] of [
      ["input:degraded", "sound", "audio"],
      ["output:unavailable", "sound", "audio"],
      ["models:missing", "sound", "models"],
      ["voice:realtime", "sound", "voice-mode"],
    ] as const) {
      const to = placeOf({ place, key })!;
      expect(to.path, key).toBe(`/settings/sound/${section}`);
      const r = resolve(to.path);
      expect(r?.route.id, key).toBe("settings");
      expect(r!.params, key).toEqual({ page: "sound", section });
    }
    // a note with no section of its own: the page
    expect(placeOf({ place: "model", key: "model:slow" })?.path).toBe("/settings/model");
    expect(placeOf({ place: "passwords", key: "passwords:locked" })?.path).toBe("/settings/passwords");
  });
  it("leads an app's sign-in note to that app's page", () => {
    expect(placeOf({ place: "apps", key: "app:github_heka" })?.path).toBe("/apps/github_heka");
    expect(resolve("/apps/github_heka")?.route.id).toBe("apps");
    // a key that isn't one app's name: the list
    expect(placeOf({ place: "apps", key: "app:../x" })?.path).toBe("/apps");
  });

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
