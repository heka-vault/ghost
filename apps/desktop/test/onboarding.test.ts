import { describe, expect, it } from "vitest";

import { answeredHere, echoOfferNow, EMPTY, type Facts, firstSentence, MEET, micHealthyOf, parseSaved, inputName, pathToMoment, progressOf, requiredAt, routeAfter, STEPS, taskNumber, TASKS, timeLeft, wakeHelp, wakeItems } from "../src/screens/onboarding/state";
import { offerOnce } from "../src/app/floating";
import { exchangeOf } from "../src/screens/onboarding/watch";


const known: Facts = { modelReady: false, modelChecked: null, speechReady: true, echo: false, apps: 0, voiceSet: false };

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
    expect(p.total).toBe(6);
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
    const off: Facts = { modelReady: null, modelChecked: null, speechReady: null, echo: null, apps: null, voiceSet: null };
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
    const all = progressOf({ modelReady: true, modelChecked: true, speechReady: true, echo: true, apps: 1, voiceSet: true }, { ...EMPTY, heard: "voice", answered: true });
    expect(all.done).toBe(6);
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

describe("an input's name", () => {
  it("reads a PipeWire node as the device", () => {
    expect(inputName("alsa_input.usb-Seeed_Studio_reSpeaker_XVF3800_4-Mic_Array_000000000000000000-00.iec958-stereo#0")).toBe("Seeed Studio reSpeaker XVF3800 4-Mic Array, channel 1");
    expect(inputName("alsa_input.usb-Shure_MV7+-00.analog-stereo")).toBe("Shure MV7+");
    expect(inputName("ghost.mic")).toBe("ghost.mic");
    expect(inputName("default source")).toBe("default source");
  });
});

describe("the path to the moment (2026-11-14)", () => {
  // A fresh install: no model yet, the speech models here, a healthy microphone.
  const fresh: Facts = { ...known, modelReady: false, micHealthy: true, micNone: false };

  it("puts the three beats first and everything else after them, optional", () => {
    expect(MEET.map((s) => s.id)).toEqual(["model", "hear", "wake"]);
    const order = STEPS.map((s) => s.id);
    for (const later of ["keeps", "voice", "app", "ask"] as const) expect(order.indexOf(later)).toBeGreaterThan(order.indexOf("wake"));
    expect(STEPS.filter((s) => s.part === "yours").map((s) => s.id)).toEqual(["voice", "app", "ask"]);
  });

  // The PR's table. A required action added before the wake beat fails here.
  it("reaches the wake beat in the stated number of actions for each newcomer", () => {
    // Claude Code signed in (ADR 0069): Use Claude Code is the one click at the model step.
    expect(pathToMoment(fresh, "claude-code")).toEqual({ steps: ["welcome", "model", "wake"], clicks: 2, fields: 0 });
    expect(requiredAt("model", "claude-code")).toEqual({ clicks: 1, fields: 0 });
    expect(pathToMoment(fresh, "found")).toEqual({ steps: ["welcome", "model", "wake"], clicks: 3, fields: 0 });
    expect(pathToMoment(fresh, "key")).toEqual({ steps: ["welcome", "model", "wake"], clicks: 3, fields: 1 });
    expect(pathToMoment(fresh, "nothing")).toEqual({ steps: ["welcome", "model", "wake"], clicks: 2, fields: 0 });
    // A model already connected (an update, a second run): Welcome straight to the moment.
    expect(pathToMoment({ ...fresh, modelReady: true }, "found")).toEqual({ steps: ["welcome", "wake"], clicks: 1, fields: 0 });
  });

  it("asks about the microphone only when it needs something", () => {
    const unhealthy = { ...fresh, micHealthy: false };
    expect(pathToMoment(unhealthy, "nothing")).toEqual({ steps: ["welcome", "model", "hear", "wake"], clicks: 3, fields: 0 });
    // Not known yet (Ghost still starting): the step is shown rather than guessed past.
    expect(routeAfter("model", { ...fresh, micHealthy: null }, { met: null, waitingForEars: false })).toBe("hear");
    // No microphone at all: the moment is typed, on the wake beat.
    expect(routeAfter("model", { ...fresh, micHealthy: null, micNone: true }, { met: null, waitingForEars: false })).toBe("wake");
    expect(requiredAt("wake", "nothing")).toEqual({ clicks: 0, fields: 0 });
  });

  it("goes back to the model after a moment with none, then on to a first real question", () => {
    // What Ghost keeps and sends comes first, always, then the model or Make it yours.
    expect(routeAfter("wake", fresh, { met: "voice", waitingForEars: false })).toBe("keeps");
    expect(routeAfter("keeps", fresh, { met: "voice", waitingForEars: false })).toBe("model");
    expect(routeAfter("model", { ...fresh, modelReady: true }, { met: "voice", waitingForEars: false })).toBe("ask");
    expect(routeAfter("keeps", { ...fresh, modelReady: true }, { met: "voice", waitingForEars: false })).toBe("voice");
  });

  it("comes back to the moment once Ghost's ears are here, from wherever they made it theirs", () => {
    const s = { met: null, waitingForEars: true };
    expect(routeAfter("voice", { ...fresh, speechReady: false }, s)).toBe("app");
    expect(routeAfter("voice", fresh, s)).toBe("wake");
    expect(routeAfter("app", fresh, s)).toBe("wake");
    // Once met, Make it yours goes on in order.
    expect(routeAfter("voice", fresh, { met: "voice", waitingForEars: true })).toBe("app");
  });

  it("counts a healthy microphone as heard, and the moment as met", () => {
    expect(progressOf({ ...fresh }, EMPTY).state.hear).toBe("done");
    expect(progressOf({ ...fresh, micHealthy: false }, EMPTY).state.hear).toBe("open");
    expect(progressOf(fresh, { ...EMPTY, met: "typed" }).met).toBe(true);
    // A typed moment is not its name heard.
    expect(progressOf(fresh, { ...EMPTY, met: "typed" }).state.wake).toBe("open");
  });

  it("reads the microphone as healthy only from Ghost's own reading", () => {
    const ok = { case: "unprotected", problem: null, fell_back: null };
    expect(micHealthyOf(ok, false)).toBe(true);
    expect(micHealthyOf(ok, true)).toBe(false);
    expect(micHealthyOf({ ...ok, problem: "It stopped hearing." }, false)).toBe(false);
    expect(micHealthyOf({ ...ok, fell_back: "USB mic isn’t there" }, false)).toBe(false);
    expect(micHealthyOf({ ...ok, case: "unknown" }, false)).toBe(false);
    expect(micHealthyOf(null, false)).toBeNull();
  });

  it("says how long the download has left, once it has a rate", () => {
    expect(timeLeft([], 100)).toBeNull();
    expect(timeLeft([{ at: 0, bytes: 0 }, { at: 10_000, bytes: 1e8 }], 2.9e9)).toBe("about 5 min left");
    expect(timeLeft([{ at: 0, bytes: 0 }, { at: 10_000, bytes: 1e9 }], 1.2e9)).toBe("under a minute left");
    expect(timeLeft([{ at: 0, bytes: 0 }, { at: 10_000, bytes: 1e6 }], 1e10)).toBe("about 27 h 47 min left");
    expect(timeLeft([{ at: 0, bytes: 5 }, { at: 10_000, bytes: 5 }], 100)).toBeNull();
  });

  it("keeps what an earlier first run stored, and the moment's own fields", () => {
    // An install that finished the first run before this change: still finished, nothing offered.
    const old = parseSaved(JSON.stringify({ step: "done", finished: true, heard: "voice" }));
    expect(old.finished).toBe(true);
    expect(old.floatOffered).toBe(false);
    expect(old.met).toBeNull();
    const s = parseSaved(JSON.stringify({ met: "typed", metWhere: "floating", floatOffered: true, waitingForEars: true, metAt: "2026-11-14T15:04:00Z" }));
    expect([s.met, s.metWhere, s.floatOffered, s.waitingForEars]).toEqual(["typed", "floating", true, true]);
    expect(parseSaved(JSON.stringify({ met: "model", metWhere: "moon" })).met).toBeNull();
  });
});

describe("the floating room's first-run offer (ADR 0059 as amended 2026-11-14)", () => {
  const off = { on: false, monitor: "other" as const };
  it("turns it on once, where the desktop can float, and never again", () => {
    const first = offerOnce(off, true);
    expect(first).toMatchObject({ on: true, offered: true });
    expect(offerOnce({ ...first!, on: false }, true)).toBeNull();
    expect(offerOnce(off, false)).toMatchObject({ on: false, offered: true });
  });
});

describe("the microphone's name", () => {
  it("calls a sound card on the board the built-in microphone", () => {
    expect(inputName("alsa_input.pci-0000_00_1f.3.analog-stereo")).toBe("the built-in microphone");
  });
});

describe("answeredHere: the moment in this window (the room view removed, 2026-11-14)", () => {
  const ok = { support: { kind: "xwayland" }, settings: { on: true }, capture: "no" as const, sharing: "none" as const };
  it("says nothing when Ghost floated, or nothing is known", () => {
    expect(answeredHere(ok)).toBeNull();
    expect(answeredHere(null)).toBeNull();
  });
  it("says why a desktop that can't float kept it here, in the shell's own words", () => {
    expect(answeredHere({ ...ok, support: { kind: "no", why: "Ghost needs XWayland to float, and this desktop isn't running it." } })).toBe(
      "Ghost answered here. On this desktop it can’t float over your work: Ghost needs XWayland to float, and this desktop isn't running it.",
    );
    expect(answeredHere({ ...ok, support: { kind: "no" } })).toBe("Ghost answered here. On this desktop it can’t float over your work.");
  });
  it("says a share kept it here, and never sends anyone to a room view", () => {
    const shared = answeredHere({ ...ok, capture: "yes", sharing: "hidden" });
    expect(shared).toBe("Ghost answered here, not over your work, because your screen is being shared.");
    expect(answeredHere({ ...ok, capture: "yes", sharing: undefined })).toBe(shared);
    expect(answeredHere({ ...ok, capture: "yes", sharing: "shown" })).toBeNull();
    for (const s of [shared, answeredHere({ ...ok, settings: { on: false } })]) expect(s).not.toMatch(/room/i);
  });
});

describe("the simplification pass (2026-11-14)", () => {
  const fresh: Facts = { ...known, modelReady: false, micHealthy: true, micNone: false };

  it("keeps the path to the wake beat as short as it was", () => {
    // The same numbers as before hiring an agent left and What Ghost keeps was added.
    expect(pathToMoment(fresh, "claude-code").clicks).toBe(2);
    expect(pathToMoment(fresh, "found").clicks).toBe(3);
    expect(pathToMoment(fresh, "key")).toMatchObject({ clicks: 3, fields: 1 });
    expect(pathToMoment(fresh, "nothing").clicks).toBe(2);
    expect(pathToMoment(fresh, "nothing").steps).not.toContain("keeps");
  });

  it("has no step for hiring an agent, and forgets one stored by an earlier first run", () => {
    expect(STEPS.map((s) => s.id)).not.toContain("agent");
    expect(TASKS.map((t) => t.id)).toEqual(["model", "hear", "wake", "voice", "app", "ask"]);
    const old = parseSaved(JSON.stringify({ step: "agent", skipped: ["agent", "app"] }));
    expect(old.step).toBeNull();
    expect(old.skipped).toEqual(["app"]);
  });

  it("puts What Ghost keeps and sends right after the wake beat, as a page, not a task", () => {
    const keeps = STEPS.find((s) => s.id === "keeps")!;
    expect(keeps.task).toBe(false);
    expect(taskNumber("keeps")).toBeNull();
    expect(MEET.map((s) => s.id)).toEqual(["model", "hear", "wake"]);
    // Skipped or met, the wake beat goes there; nothing else does.
    expect(routeAfter("wake", fresh, { met: null, waitingForEars: false })).toBe("keeps");
    expect(STEPS.filter((s) => routeAfter(s.id, fresh, { met: null, waitingForEars: false }) === "keeps").map((s) => s.id)).toEqual(["wake"]);
    expect(progressOf(fresh, EMPTY).state.keeps).toBe("open");
    expect(progressOf(fresh, { ...EMPTY, keepsSeen: true }).state.keeps).toBe("done");
    expect(parseSaved(JSON.stringify({ keepsSeen: true })).keepsSeen).toBe(true);
  });

  it("offers the echo canceller on the wake beat only on open speakers with nothing taking Ghost's voice out", () => {
    const m = (c: string, problem: string | null = null) => ({ case: c, problem });
    expect(echoOfferNow(m("unprotected"), false)).toBe(true);
    expect(echoOfferNow(m("unprotected"), true)).toBe(false);
    expect(echoOfferNow(m("unprotected", "It stopped hearing."), false)).toBe(false);
    for (const c of ["on-chip", "headset", "host-canceller", "unknown"]) expect(echoOfferNow(m(c), false)).toBe(false);
    expect(echoOfferNow(null, false)).toBe(false);
  });

  // The wake beat's count of things under its title. A control or a sentence added fails here.
  it("shows three things on the wake beat for a fresh install, and at most five anywhere", () => {
    const base = { floatKnown: true, mic: true, echoNow: false, keepingTakes: false };
    expect(wakeItems(base)).toEqual(["float", "mic", "other-ways"]);
    expect(wakeItems({ ...base, echoNow: true })).toEqual(["float", "mic", "echo", "other-ways"]);
    expect(wakeItems({ ...base, keepingTakes: true })).toEqual(["float", "mic", "takes", "other-ways"]);
    expect(wakeItems({ floatKnown: true, mic: true, echoNow: true, keepingTakes: true })).toHaveLength(5);
  });
});
