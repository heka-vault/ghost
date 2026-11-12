import { describe, expect, it } from "vitest";

import { Follow, INTENT_MS } from "../src/screens/session/follow";

/** A panel 600 high over `height` of content, scrolled to `top`. */
const at = (top: number, height = 2000) => ({ top, height, client: 600 });

describe("the thread follows the newest turn", () => {
  it("starts pinned and stays pinned while the operator is at the end", () => {
    const f = new Follow();
    expect(f.pinned).toBe(true);
    f.scrolled(at(1400), 0);
    expect(f.pinned).toBe(true);
    expect(f.onNew()).toBe("follow");
  });

  it("content shrinking above (the browser clamping the scroll) never lets go", () => {
    const f = new Follow();
    f.scrolled(at(1400), 0);
    // A working row collapsed, held pieces became one turn: the content is 300 shorter and the
    // browser pulled the scroll up with it. No input from the operator.
    f.scrolled(at(1100, 1700), 1000);
    expect(f.pinned).toBe(true);
    // Now short of the end, by a growth that came with no scroll at all: still followed.
    f.scrolled(at(1100, 2100), 1100);
    expect(f.pinned).toBe(true);
    expect(f.onNew()).toBe("follow");
  });

  it("the room view scrolling the panel to its top doesn't let go, so leaving it returns to the end", () => {
    const f = new Follow();
    f.scrolled(at(1400), 0);
    f.scrolled(at(0), 20_000); // scrollPanelToTop() as room view comes on, nobody touching
    expect(f.pinned).toBe(true);
    expect(f.onNew()).toBe("follow");
  });

  it("only the operator's own scroll up lets go, and then a new reply is marked", () => {
    const f = new Follow();
    f.scrolled(at(1400), 0);
    f.intent(5000); // a wheel up, a key, a drag on the scrollbar
    f.scrolled(at(900), 5000 + 30);
    expect(f.pinned).toBe(false);
    expect(f.onNew()).toBe("mark");
  });

  it("an intent too long ago does not count", () => {
    const f = new Follow();
    f.scrolled(at(1400), 0);
    f.intent(1000);
    f.scrolled(at(900), 1000 + INTENT_MS + 1);
    expect(f.pinned).toBe(true);
  });

  it("scrolling back to the end pins again; a new session, sending and a waiting change always follow", () => {
    const f = new Follow();
    f.scrolled(at(1400), 0);
    f.intent(5);
    f.scrolled(at(500), 10);
    expect(f.pinned).toBe(false);
    f.scrolled(at(1380), 20); // within reach of the end
    expect(f.pinned).toBe(true);

    f.intent(100);
    f.scrolled(at(500), 110);
    expect(f.pinned).toBe(false);
    expect(f.onNew({ sent: true })).toBe("follow");
    f.scrolled(at(1400), 150);
    f.intent(200);
    f.scrolled(at(500), 210);
    expect(f.onNew({ asked: true })).toBe("follow");
    f.scrolled(at(1400), 250);
    f.intent(300);
    f.scrolled(at(500), 310);
    expect(f.pinned).toBe(false);
    f.newSession();
    expect(f.pinned).toBe(true);
  });
});
