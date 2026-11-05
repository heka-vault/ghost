import type { Turn } from "@ghost/client-ts";
import { createVoiceLevel } from "@ghost/ui";
import { describe, expect, it } from "vitest";

import { agentPath } from "../src/screens/conversations/url";
import { buildHistory } from "../src/screens/session/transcript";
import { speechEnvelope } from "../src/ui/VoiceWave";

const turn = (session_id: string, role: Turn["role"], text: string, at: string): Turn => ({ session_id, role, text, at, via: "voice" });
const lines = [
  { slug: "ghost", name: "Ghost", aliases: [] },
  { slug: "accounting", name: "Accounting", aliases: ["books"] },
];

describe("home is Ghost's page", () => {
  it("sends Ghost's row home and every other agent to its page", () => {
    expect(agentPath("ghost")).toBe("/");
    expect(agentPath("accounting")).toBe("/agents/accounting");
  });

  it("starts each stored session with a dated caption and keeps every reply Ghost's", () => {
    const items = buildHistory(
      [
        turn("a", "user", "anything new?", "2026-11-01T10:00:00Z"),
        turn("a", "assistant", "Two issues.", "2026-11-01T10:00:02Z"),
        turn("a", "assistant", "Transferring you to Accounting.", "2026-11-01T10:00:05Z"),
        turn("a", "assistant", "Back with you.", "2026-11-01T10:05:00Z"),
        turn("b", "user", "hi", "2026-11-02T09:00:00Z"),
      ],
      lines,
    );
    expect(items.map((i) => i.kind)).toEqual(["started", "you", "ghost", "transfer", "ghost", "started", "you"]);
    expect(items[0]).toMatchObject({ kind: "started", dated: true });
    // After a transfer sentence the stored replies are still Ghost's: the agent's live in its own transcript.
    expect(items[4]).toMatchObject({ kind: "ghost", slug: "ghost" });
    expect(items[3]).toMatchObject({ kind: "transfer", to: "accounting" });
    expect(new Set(items.map((i) => i.key)).size).toBe(items.length);
    expect(items.every((i) => i.key.startsWith("h"))).toBe(true);
  });
});

describe("Ghost's voice level", () => {
  it("clamps, notifies and unsubscribes", () => {
    const v = createVoiceLevel();
    const seen: number[] = [];
    const off = v.subscribe((l) => seen.push(l));
    v.set(0.4);
    v.set(3);
    v.set(-1);
    v.set(Number.NaN);
    off();
    v.set(0.5);
    expect(seen).toEqual([0.4, 1, 0, 0]);
    expect(v.get().level).toBe(0.5);
  });

  it("has a stand-in voice with words, gaps and pauses, always within 0 to 1", () => {
    const samples = Array.from({ length: 200 }, (_, i) => speechEnvelope(i * 0.05));
    expect(samples.every((s) => s >= 0 && s <= 1)).toBe(true);
    expect(samples.some((s) => s > 0.5)).toBe(true);
    expect(samples.filter((s) => s === 0).length).toBeGreaterThan(10);
  });
});
