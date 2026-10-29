import { describe, expect, it } from "vitest";

import { pairLink, parsePairLink } from "../src";

describe("the pairing link", () => {
  // The daemon's own spelling (ghostd::phone::pairing builds the same string).
  const daemon = "ghost://pair?code=482913&address=192.168.1.20%3A8765&address=10.8.0.4%3A8765&address=%5Bfd00%3A%3A4%5D%3A8765";
  const addresses = ["192.168.1.20:8765", "10.8.0.4:8765", "[fd00::4]:8765"];

  it("is built the way the daemon builds it", () => {
    expect(pairLink("482913", addresses)).toBe(daemon);
  });

  it("reads back the code and every address, in order", () => {
    expect(parsePairLink(daemon)).toEqual({ code: "482913", addresses });
    expect(parsePairLink(`  ${daemon}\n`)).toEqual({ code: "482913", addresses });
    expect(parsePairLink("GHOST://pair/?code=000001&address=desk.lan:8765&v=2")).toEqual({ code: "000001", addresses: ["desk.lan:8765"] });
  });

  it("is nothing but a pairing link", () => {
    for (const bad of [
      "",
      "482913",
      "https://example.com/pair?code=482913&address=1.2.3.4:8765",
      "ghost://home?code=482913&address=1.2.3.4:8765",
      "ghost://pair?code=48291&address=1.2.3.4:8765",
      "ghost://pair?code=4829134&address=1.2.3.4:8765",
      "ghost://pair?code=482913",
      "ghost://pair?code=482913&code=111111&address=1.2.3.4:8765",
      "ghost://pair?code=482913&address=1.2.3.4",
      "ghost://pair?code=482913&address=http%3A%2F%2F1.2.3.4%3A8765",
      "ghost://pair?code=482913&address=1.2.3.4:8765%2Fx",
      "ghost://pair?code=482913&address=%E0%A4%A",
      `ghost://pair?code=482913${"&address=1.2.3.4:8765".repeat(9)}`,
    ]) {
      expect(parsePairLink(bad), bad).toBeNull();
    }
  });
});
