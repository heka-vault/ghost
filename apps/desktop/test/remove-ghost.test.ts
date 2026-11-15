import { describe, expect, it } from "vitest";

import { uninstallView } from "../src/design/mock/uninstall";
import { chosenAtFirst, humanSize } from "../src/screens/settings/remove";

describe("Remove Ghost from this computer (#147)", () => {
  it("reads sizes the way ghostd uninstall prints them", () => {
    expect(humanSize(0)).toBe("nothing");
    expect(humanSize(512)).toBe("512 bytes");
    expect(humanSize(2_842_118_400)).toBe("2.8 GB");
    expect(humanSize(340_000_000)).toBe("340 MB");
  });

  it("never chooses the person's data or passwords for them", () => {
    const v = uninstallView();
    const chosen = chosenAtFirst(v);
    expect(chosen.has("data")).toBe(false);
    expect(chosen.has("vault")).toBe(false);
    expect([...chosen].sort()).toEqual(["app-storage", "other-tools", "running", "speech-models"]);
  });

  it("names the two paths the agent guide's undo missed", () => {
    const paths = uninstallView().categories.flatMap((c) => c.items.flatMap((i) => i.paths));
    expect(paths).toContain("~/.config/autostart/heka-ghost-desktop.desktop");
    expect(paths).toContain("~/.local/share/dev.heka.ghost");
  });
});
