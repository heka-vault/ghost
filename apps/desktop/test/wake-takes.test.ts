import type { WakeTakeView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { afterDelete, bytesText, curvePath, keyAction, move, nextAfterLabel, tally } from "../src/screens/hear/wakeTakes";

function take(id: string, extra: Partial<WakeTakeView> = {}): WakeTakeView {
  return {
    id,
    at: "2026-11-13T09:00:00Z",
    outcome: "accepted",
    model: "hey_ghost",
    score: 0.5,
    threshold: 0.6,
    input: "ghost.mic",
    array: null,
    secs: 13.5,
    hit_at: 12,
    scores: [0.1, 0.5],
    scores_end: 12,
    reading: null,
    label: null,
    bytes: 1000,
    ...extra,
  };
}

describe("labelling a take takes one key", () => {
  it("maps the keys", () => {
    expect(keyAction({ key: "y" })).toEqual({ kind: "label", label: "me" });
    expect(keyAction({ key: "N" })).toEqual({ kind: "label", label: "not" });
    expect(keyAction({ key: "b" })).toEqual({ kind: "label", label: "me_missed_before" });
    expect(keyAction({ key: "u" })).toEqual({ kind: "label", label: null });
    expect(keyAction({ key: " " })).toEqual({ kind: "play", array: false });
    expect(keyAction({ key: "a" })).toEqual({ kind: "play", array: true });
    expect(keyAction({ key: "ArrowDown" })).toEqual({ kind: "move", by: 1 });
    expect(keyAction({ key: "k" })).toEqual({ kind: "move", by: -1 });
    expect(keyAction({ key: "Delete" })).toEqual({ kind: "delete" });
    expect(keyAction({ key: "q" })).toBeNull();
  });

  it("leaves a key with a modifier to the app (copy, reload)", () => {
    expect(keyAction({ key: "y", ctrlKey: true })).toBeNull();
    expect(keyAction({ key: "n", metaKey: true })).toBeNull();
    expect(keyAction({ key: "x", altKey: true })).toBeNull();
  });

  it("goes to the next unlabelled take, wrapping to an earlier one", () => {
    const list = [take("a", { label: "me" }), take("b"), take("c", { label: "not" }), take("d")];
    expect(nextAfterLabel(list, 1)).toBe(3);
    expect(nextAfterLabel(list, 3)).toBe(1);
    const done = list.map((t) => ({ ...t, label: "me" as const }));
    expect(nextAfterLabel(done, 1)).toBe(2);
    expect(nextAfterLabel(done, 3)).toBe(3);
    expect(nextAfterLabel([], 0)).toBe(-1);
  });

  it("keeps a selection in the list when a take is deleted or the arrows run off it", () => {
    expect(afterDelete(1, 0)).toBe(-1);
    expect(afterDelete(3, 2)).toBe(1);
    expect(afterDelete(3, 0)).toBe(0);
    expect(move(3, 2, 1)).toBe(2);
    expect(move(3, 0, -1)).toBe(0);
    expect(move(0, 0, 1)).toBe(-1);
  });
});

describe("what the retraining learns from", () => {
  const list = [
    take("me", { label: "me" }),
    take("before", { label: "me_missed_before" }),
    take("not", { label: "not", outcome: "refused" }),
    take("woke"),
    take("refused", { outcome: "refused" }),
  ];

  it("counts only the operator's labels by default", () => {
    expect(tally(list, false)).toEqual({ me: 1, missedBefore: 1, not: 1, unlabelled: 2, positives: 2, negatives: 1 });
  });

  it("counts an unlabelled wake as theirs only when they said so, and never an unlabelled refused hit", () => {
    expect(tally(list, true).positives).toBe(3);
    expect(tally(list, true).negatives).toBe(1);
  });
});

describe("the numbers shown", () => {
  it("says sizes plainly", () => {
    expect(bytesText(512)).toBe("512 B");
    expect(bytesText(2048)).toBe("2 KB");
    expect(bytesText(1.5 * 1024 * 1024)).toBe("1.5 MB");
    expect(bytesText(300 * 1024 * 1024)).toBe("300 MB");
    expect(bytesText(1024 * 1024 * 1024)).toBe("1.0 GB");
  });

  it("draws the score curve 0 at the bottom", () => {
    expect(curvePath([0, 1], 10, 10)).toBe("M0.0 10.0 L10.0 0.0");
    expect(curvePath([0.5], 10, 10)).toBe("");
  });
});
