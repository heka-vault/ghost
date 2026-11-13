import { describe, expect, it } from "vitest";

import { resolve } from "../src/app/routes";

describe("the Hearing test's tabs", () => {
  it("opens every tab by its address, Wake takes included", () => {
    for (const tab of ["sessions", "runs", "plan", "wake"]) expect(resolve(`/hear/${tab}`)?.route.id, tab).toBe("hear");
    expect(resolve("/hear/wake/x")).toBeNull();
  });
});
