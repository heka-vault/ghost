import { describe, expect, it } from "vitest";

import { EMPTY, type Facts, firstSentence, parseSaved, progressOf, STEPS, taskNumber, TASKS, wakeHelp } from "../src/screens/onboarding/state";
import { exchangeOf } from "../src/screens/onboarding/watch";


const known: Facts = { modelReady: false, modelChecked: null, speechReady: true, echo: false, apps: 0, agents: 0, voiceSet: false };

describe("the first run's progress", () => {
  it("numbers the tasks the count counts, and not the welcome and done pages", () => {
    const numbers = STEPS.map((s) => taskNumber(s.id)).filter((n) => n !== null);
    expect(numbers).toEqual(TASKS.map((_, i) => i + 1));
    expect(Math.max(...numbers)).toBe(progressOf(known, EMPTY).total);
    expect(taskNumber("welcome")).toBeNull();
    expect(taskNumber("done")).toBeNull();
  });

  it("counts only what Ghost reports as done", () => {
    const p = progressOf(known, EMPTY);
    expect(p.done).toBe(0);
    expect(p.total).toBe(7);
    expect(p.next).toBe("model");
    expect(p.canTalk).toBe(false);
  });

  it("never counts a click as hearing its name", () => {
    expect(progressOf(known, { ...EMPTY, heard: "click" }).state.wake).toBe("open");
    const heard = progressOf(known, { ...EMPTY, heard: "voice" });
    expect(heard.state.wake).toBe("done");
    // A wake on the operator's voice proves the microphone, echo cancellation or not.
    expect(heard.state.hear).toBe("done");
  });

  it("needs the speech models to call hearing done", () => {
    expect(progressOf({ ...known, speechReady: false, echo: true }, EMPTY).state.hear).toBe("open");
    expect(progressOf({ ...known, echo: true }, EMPTY).state.hear).toBe("done");
  });

  it("says unknown, not open, when Ghost isn't answering", () => {
    const off: Facts = { modelReady: null, modelChecked: null, speechReady: null, echo: null, apps: null, agents: null, voiceSet: null };
    const p = progressOf(off, EMPTY);
    expect(p.state.model).toBe("unknown");
    expect(p.state.app).toBe("unknown");
    // What the operator did here is still known.
    expect(p.state.ask).toBe("open");
  });

  it("goes to the first open task, then the skipped ones", () => {
    const p = progressOf({ ...known, modelReady: true }, { ...EMPTY, skipped: ["hear"] });
    expect(p.state.hear).toBe("skipped");
    expect(p.next).toBe("wake");
    const all = progressOf({ modelReady: true, modelChecked: true, speechReady: true, echo: true, apps: 1, agents: 1, voiceSet: true }, { ...EMPTY, heard: "voice", answered: true });
    expect(all.done).toBe(7);
    expect(all.next).toBe("done");
    expect(all.state.done).toBe("done");
    expect(all.canTalk).toBe(true);
  });

  it("resumes from what was stored, and survives junk", () => {
    expect(parseSaved(null)).toEqual(EMPTY);
    expect(parseSaved("{not json")).toEqual(EMPTY);
    const s = parseSaved(JSON.stringify({ step: "wake", skipped: ["app", "nope"], heard: "voice", heardAt: "2026-11-12T15:00:00Z", answered: true, voiceChosen: "yes" }));
    expect(s.step).toBe("wake");
    expect(s.skipped).toEqual(["app"]);
    expect(s.heard).toBe("voice");
    expect(s.answered).toBe(true);
    expect(s.voiceChosen).toBe(false);
  });
});

describe("what to try when Ghost doesn't hear its name", () => {
  it("puts listening first when it is off, and always ends with the Hearing test", () => {
    const h = wakeHelp({ listeningOff: true, device: "ghost.mic", echo: true, fuzzy: 0.7, phrases: ["hey ghost"] });
    expect(h.map((x) => x.id)).toEqual(["listen", "hearing"]);
  });
  it("suggests the microphone, a looser match and the other phrase from what is true", () => {
    const h = wakeHelp({ listeningOff: false, device: "USB Mic", echo: false, fuzzy: 0.9, phrases: ["hey ghost", "ok ghost"] });
    expect(h.map((x) => x.id)).toEqual(["mic", "fuzzy", "other", "hearing"]);
    expect(h[2].text).toContain("“Ok Ghost”");
  });
});

describe("the first question and its answer", () => {
  const at = (s: number) => new Date(Date.parse("2026-11-12T15:00:00Z") + s * 1000).toISOString();
  const since = Date.parse("2026-11-12T15:00:00Z");
  it("ignores turns from before the step opened", () => {
    const turns = [
      { role: "user", text: "old", via: "voice", session_id: "a", at: at(-30) },
      { role: "assistant", text: "old answer", via: "voice", session_id: "a", at: at(-29) },
    ];
    expect(exchangeOf(turns, since)).toBeNull();
  });
  it("pairs the question with the next answer in its session", () => {
    const turns = [
      { role: "user", text: "What can you do?", via: "voice", session_id: "b", at: at(2) },
      { role: "assistant", text: "elsewhere", via: "voice", session_id: "c", at: at(3) },
      { role: "assistant", text: "I read your apps and act on them.", via: "voice", session_id: "b", at: at(4) },
    ];
    expect(exchangeOf(turns, since)).toEqual({ question: "What can you do?", answer: "I read your apps and act on them.", spoken: true });
  });
  it("waits for the answer", () => {
    expect(exchangeOf([{ role: "user", text: "hi", via: "text", session_id: "b", at: at(1) }], since)).toBeNull();
  });
});

describe("a catalog entry's description", () => {
  it("keeps the maker's first sentence", () => {
    expect(firstSentence("Read and write issues on GitHub. Also pull requests.")).toBe("Read and write issues on GitHub.");
    expect(firstSentence("x".repeat(200)).endsWith("…")).toBe(true);
  });
});
