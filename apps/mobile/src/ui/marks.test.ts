/**
 * The phone's brand files come from the one drawing (packages/ui/src/mark.ts) and are written by
 * `make marks` (`marks.mjs mobile`): these hold the committed files to it.
 */
import { readFileSync } from "node:fs";
import { join } from "node:path";

import { NIGHT, notificationVector, wispPath } from "@ghost/ui";
import { describe, expect, it } from "vitest";

const MOBILE = join(__dirname, "..", "..");
const read = (p: string) => readFileSync(join(MOBILE, p), "utf8");

describe("the phone's brand files", () => {
  it("draws the status-bar icon from the generator, the small hem with the nonzero rule", () => {
    const xml = read("modules/ghost-call/android/src/main/res/drawable/ghost_link.xml");
    expect(xml).toBe(notificationVector());
    expect(xml).toContain(wispPath({ mitts: false, small: true }));
    expect(xml).not.toMatch(/evenOdd/i);
  });

  it("puts the launcher icon and the splash on the night, with no indigo anywhere", () => {
    const app = read("app.json");
    expect(app).not.toMatch(/5560e8|4c57e0/i);
    const config = JSON.parse(app).expo;
    expect(config.android.adaptiveIcon.backgroundColor).toBe(NIGHT);
    expect(config.android.adaptiveIcon.monochromeImage).toBe("./assets/android-icon-monochrome.png");
    const splash = config.plugins.find((p: unknown) => Array.isArray(p) && p[0] === "expo-splash-screen")?.[1];
    expect(splash).toMatchObject({ image: "./assets/splash-icon.png", backgroundColor: NIGHT });
  });
});
