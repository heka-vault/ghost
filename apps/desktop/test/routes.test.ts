import { describe, expect, it } from "vitest";

import { ROUTES, redirectFor, resolve } from "../src/app/routes";

describe("the Hearing test's tabs", () => {
  it("opens every tab by its address, Wake takes included", () => {
    for (const tab of ["sessions", "runs", "plan", "wake"]) expect(resolve(`/hear/${tab}`)?.route.id, tab).toBe("hear");
    expect(resolve("/hear/wake/x")).toBeNull();
  });
});

describe("addresses that moved on 2026-11-14", () => {
  it("lands Watching, its old sections and the departments on On its own", () => {
    for (const old of ["/watching", "/watching/schedules", "/departments", "/departments/legal", "/departments/legal/watches"]) {
      expect(redirectFor(old), old).toBe("/on-its-own");
      expect(resolve(old)?.route.id, old).toBe("watching");
    }
    expect(redirectFor("/on-its-own")).toBeNull();
  });
  it("keeps the Hearing test at its own address, off the rail", () => {
    expect(resolve("/hear")?.route.id).toBe("hear");
    expect(resolve("/hear/wake")?.route.id).toBe("hear");
    expect(ROUTES.find((r) => r.id === "hear")?.nav).toBeUndefined();
  });
  it("has four places on the rail: Today, On its own, Apps and Settings", () => {
    expect(ROUTES.filter((r) => r.nav && r.nav.group !== "prototype").map((r) => r.nav!.label)).toEqual(["Today", "On its own", "Apps", "Settings"]);
  });
});
