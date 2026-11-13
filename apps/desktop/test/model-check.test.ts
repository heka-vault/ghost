import type { ModelCheckModel, ModelCheckView } from "@ghost/client-ts";
import { describe, expect, it } from "vitest";

import { verdictOf } from "../src/screens/settings/check";

const yes = { state: "yes" } as ModelCheckModel["tools"];
const model = (over: Partial<ModelCheckModel> = {}): ModelCheckModel =>
  ({
    role: "main",
    model: "m",
    reachable: true,
    samples: 3,
    first_word: { median_ms: 700, worst_ms: 900 },
    streaming: yes,
    tools: yes,
    caching: yes,
    thinking: yes,
    ...over,
  }) as ModelCheckModel;
const check = (models: ModelCheckModel[], advice: string | null = null) => ({ models, advice, checked_at: "2026-11-12T15:00:00Z", where: "x" }) as unknown as ModelCheckView;

describe("the model check as a verdict", () => {
  it("is ready to talk when the first word is quick", () => {
    const v = verdictOf(check([model()]));
    expect(v.tone).toBe("ok");
    expect(v.title).toBe("Ready to talk: first word in 700 ms");
    expect(v.help).toBeNull();
  });
  it("says what would help when it can't reach the model", () => {
    const v = verdictOf(check([model({ reachable: false, error: "connection refused" })]));
    expect(v.tone).toBe("danger");
    expect(v.help).toMatch(/^Connection refused\. Check the address/);
  });
  it("calls a model without tool calls a failure, before speed", () => {
    expect(verdictOf(check([model({ tools: { state: "no" } as ModelCheckModel["tools"] })])).title).toBe("It answers, but can’t use tools");
  });
  it("judges speed by the fast model when there is one, and never says it in amber", () => {
    const slow = verdictOf(check([model({ first_word: { median_ms: 2400, worst_ms: 3000 } })], "Try a provider nearer you."));
    expect(slow.tone).toBe("danger");
    expect(slow.help).toBe("Try a provider nearer you.");
    const fronted = verdictOf(check([model({ first_word: { median_ms: 2400, worst_ms: 3000 } }), model({ role: "fast", first_word: { median_ms: 500, worst_ms: 600 } })]));
    expect(fronted.tone).toBe("ok");
    expect(fronted.firstWordMs).toBe(500);
  });
  it("says a missing stream plainly, as information", () => {
    expect(verdictOf(check([model({ streaming: { state: "no" } as ModelCheckModel["tools"] })])).tone).toBe("info");
  });
});
