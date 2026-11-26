import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

import { GhostClient, type DictationKeyView, type DictationLiveView, type GhostEvent } from "@ghost/client-ts";
import { createDictationLive } from "@ghost/ui";

import { dictationSentence, dictationShown, dictationTitle, goneAt, isEnded, lingerMs, nextSeen, splitWords, toneOf, wordsShown } from "../src/app/dictation";
import { MockDaemon } from "../src/design/mock/daemon";
import { keyViewFor } from "../src/design/mock/dictation";
import { composeFixtures } from "../src/design/mock/fixtures";
import { scenarioByName } from "../src/design/mock/scenarios";
import { labelOf, sceneOf } from "../src/floating/scene";
import { DictationRows } from "../src/screens/settings/Dictation";
import { ANOTHER_OPENROUTER, BINDING, HOLD_OR_TAP, OPENROUTER_MODELS, keyBusy, keyName, keyNotes, keyShown, tidyBody, tidyOptions, tidyValue, tidyWhere } from "../src/screens/settings/dictation";

// settings/model.ts reads the address at load (its draft's key); Node has no window.
vi.hoisted(() => {
  const g = globalThis as { window?: unknown };
  g.window ??= { location: { search: "", hash: "" } };
});

const view = (o: Partial<DictationLiveView> = {}): DictationLiveView => ({ scene: "listening", key: true, level: null, text: null, seconds: 1, limit_secs: 120, sentence: null, ...o });

describe("a dictation's words (ADR 0077 as amended 2026-11-25)", () => {
  it("says its state in a few words, then how it ended", () => {
    expect(dictationTitle(view({ scene: "starting" }))).toBe("Listening");
    expect(dictationTitle(view({ scene: "listening" }))).toBe("Listening");
    expect(dictationTitle(view({ scene: "writing" }))).toBe("Writing it down");
    expect(dictationTitle(view({ scene: "typed", sentence: "Typed 42 characters." }))).toBe("Typed");
    expect(dictationTitle(view({ scene: "clipboard" }))).toBe("On your clipboard");
    expect(dictationTitle(view({ scene: "nothing_heard" }))).toBe("Nothing heard");
    expect(dictationTitle(view({ scene: "stopped" }))).toBe("Stopped");
  });

  it("adds Ghost's sentence only when it says more: what to do, or why it stopped", () => {
    const clip = "Some characters can't be typed here. On your clipboard: 12 characters. Press Ctrl+V.";
    expect(dictationSentence(view({ scene: "clipboard", sentence: clip }))).toBe(clip);
    expect(dictationSentence(view({ scene: "clipboard" }))).toContain("Ctrl+V");
    // Typed: the text is where the cursor was; “Typed” is all.
    expect(dictationSentence(view({ scene: "typed", sentence: "Typed 42 characters." }))).toBeNull();
    // A sentence that only repeats the title is said once.
    expect(dictationSentence(view({ scene: "nothing_heard", sentence: "Nothing heard." }))).toBeNull();
    expect(dictationSentence(view({ scene: "stopped", sentence: "Stopped: a session opened." }))).toBe("Stopped: a session opened.");
    // Never while it goes on.
    expect(dictationSentence(view({ scene: "listening", sentence: "x" }))).toBeNull();
  });

  it("draws its light by state", () => {
    expect(toneOf("starting")).toBe("listening");
    expect(toneOf("writing")).toBe("writing");
    for (const s of ["typed", "clipboard", "nothing_heard", "stopped"] as const) expect(toneOf(s)).toBe("done");
  });

  it("keeps the newest words when they don't fit, cut at a word", () => {
    expect(wordsShown(null)).toBe("");
    expect(wordsShown("  move   the  review ")).toBe("move the review");
    const long = Array.from({ length: 60 }, (_, i) => `word${i}`).join(" ");
    const shown = wordsShown(long, 40);
    expect(shown.startsWith("…")).toBe(true);
    expect(shown.endsWith("word59")).toBe(true);
    expect(shown.length).toBeLessThanOrEqual(41);
    expect(shown.slice(1).split(" ").every((w) => /^word\d+$/.test(w))).toBe(true);
  });
});

describe("how long a dictation stays on screen", () => {
  it("goes a moment after it typed or heard nothing, longer when there is something to do or read", () => {
    expect(lingerMs("typed")).toBe(1_000);
    expect(lingerMs("nothing_heard")).toBe(1_000);
    expect(lingerMs("clipboard")).toBe(3_000);
    expect(lingerMs("stopped")).toBe(3_000);
    for (const s of ["starting", "listening", "writing"] as const) {
      expect(lingerMs(s)).toBeNull();
      expect(isEnded(s)).toBe(false);
    }
  });

  it("stays while it goes on, then for its moment from the first end seen", () => {
    let seen = nextSeen(null, view({ scene: "starting" }), 0);
    expect(goneAt(seen)).toBeNull();
    expect(dictationShown(seen, 1e9)).toBe(true);
    seen = nextSeen(seen, view({ scene: "writing" }), 5_000);
    expect(dictationShown(seen, 1e9)).toBe(true);
    seen = nextSeen(seen, view({ scene: "clipboard" }), 6_000);
    expect(goneAt(seen)).toBe(9_000);
    // The same end again doesn't restart the clock.
    seen = nextSeen(seen, view({ scene: "clipboard" }), 8_000);
    expect(goneAt(seen)).toBe(9_000);
    expect(dictationShown(seen, 8_999)).toBe(true);
    expect(dictationShown(seen, 9_000)).toBe(false);
    // Typed goes after a second.
    const typed = nextSeen(null, view({ scene: "typed" }), 100);
    expect(dictationShown(typed, 1_099)).toBe(true);
    expect(dictationShown(typed, 1_100)).toBe(false);
    // A new dictation brings it back at once.
    expect(dictationShown(nextSeen(seen, view({ scene: "starting" }), 20_000), 20_000)).toBe(true);
    expect(dictationShown(null, 0)).toBe(false);
    expect(nextSeen(seen, null, 0)).toBeNull();
  });
});

describe("the dictation's store: in memory, the words kept only while it is on screen", () => {
  it("keeps the last level when a reading has none, and the words through writing and the end", () => {
    const d = createDictationLive();
    const changes = vi.fn();
    const levels = vi.fn();
    d.subscribe(changes);
    d.subscribeLevel(levels);
    d.push(view({ scene: "starting" }));
    expect(d.get()?.text).toBeNull();
    d.push(view({ level: 0.5 }));
    d.push(view({ level: 0.6, text: "move the" }));
    d.push(view({ level: 0.4, text: "move the" }));
    expect(levels).toHaveBeenCalledTimes(3);
    // A level alone is no change for the drawing; the words are.
    expect(changes).toHaveBeenCalledTimes(3);
    d.push(view({ scene: "writing", level: null, text: null }));
    expect(d.get()).toMatchObject({ scene: "writing", text: "move the", level: 0.4 });
    d.push(view({ scene: "typed", text: null, sentence: "Typed 8 characters." }));
    expect(d.get()?.text).toBe("move the");
    // The next dictation starts with none of the last one's words.
    d.push(view({ scene: "starting" }));
    expect(d.get()?.text).toBeNull();
    expect(d.level().level).toBe(0);
    // A dropped connection forgets it.
    d.reset();
    expect(d.get()).toBeNull();
  });
});

describe("the floating scene of a dictation", () => {
  const base = { capture: null, phase: "passive", ending: false, youTalking: false, pending: false, question: false } as const;
  it("is its own scene when it floats, and the line when it doesn't", () => {
    expect(sceneOf({ ...base, posture: "float", dictating: true })).toBe("dictation");
    expect(sceneOf({ ...base, posture: "desk", dictating: true })).toBe("line");
    // A share Ghost is kept out of hides it like anything else.
    expect(sceneOf({ ...base, posture: "float", dictating: true, sharing: "hidden" })).toBe("hidden");
  });
  it("is labelled in the dictation's own words", () => {
    const o = { agent: "Ghost", using: null, asker: null, micOff: false };
    expect(labelOf("dictation", { ...o, dictation: "Writing it down" })).toBe("Writing it down");
    expect(labelOf("dictation", o)).toBe("Listening");
  });
});

describe("Settings › Sound › Dictation", () => {
  const ready: DictationKeyView = {
    on: true,
    state: "ready",
    key: "Press F9",
    typing: "allowed",
    tidy: true,
    tidy_source: "subscription",
    tidy_with: "Claude Code",
    tidy_model: "haiku",
    tidy_model_default: "anthropic/claude-haiku-5.5",
    openrouter: false,
    subscription: true,
  };
  const rows = (v: DictationKeyView, pending: "on" | "off" | "tidy" | "change" | null = null, keyControl: string | null = null) =>
    renderToStaticMarkup(createElement(DictationRows, { view: v, pending, online: true, onTurn: () => undefined, onTidy: () => undefined, onChange: () => undefined, keyControl: keyControl && createElement("p", null, keyControl) }));

  it("shows the key in GNOME's words, short, with Change key, and hold or tap", () => {
    expect(keyName("Press F9")).toBe("F9");
    expect(keyName("Press Ctrl+Alt+D")).toBe("Ctrl+Alt+D");
    expect(keyName("Press F9 or F10")).toBe("Press F9 or F10");
    expect(keyName("Super+D")).toBe("Super+D");
    expect(keyName(null)).toBeNull();
    const html = rows(ready);
    expect(html).toContain("Hold a key to dictate");
    expect(html).toContain(">F9<");
    expect(html).toContain("Change key");
    expect(html).toContain(HOLD_OR_TAP.replaceAll("’", "&#x27;").slice(0, 20));
    expect(keyShown(ready)).toBe(true);
    expect(keyShown({ ...ready, state: "binding" })).toBe(false);
  });

  it("waits for GNOME's dialogs while it turns on, and says so", () => {
    expect(keyBusy({ state: "binding" }, false)).toBe(true);
    expect(keyBusy({ state: "off" }, true)).toBe(true);
    expect(keyBusy({ state: "ready" }, false)).toBe(false);
    const html = rows({ ...ready, on: false, state: "off", key: null }, "on");
    expect(html).toContain(BINDING);
    expect(html).not.toContain("Change key");
  });

  it("says where the tidy sends the words and about how long, never a password", () => {
    expect(tidyWhere(ready)).toBe("Your words go to Anthropic through your Claude Code, about 3 s.");
    expect(tidyWhere({ tidy_source: "openrouter", tidy_with: "OpenRouter", openrouter: true })).toBe("Your words go to OpenRouter, under a second.");
    expect(tidyWhere({ tidy_source: "openrouter", tidy_with: null, openrouter: false })).toBe("Add your OpenRouter key below to tidy there. Until then Ghost types what it heard.");
    expect(tidyWhere({ tidy_source: "off", tidy_with: null, openrouter: true })).toBe("Ghost types what it heard, as it heard it.");
    expect(tidyWhere({ tidy_source: "subscription", tidy_with: "Anthropic", openrouter: false })).toBe("Your words go to Anthropic to be tidied, within 3 s.");
    expect(tidyWhere({ tidy_source: "subscription", tidy_with: null, openrouter: false })).toBe("No model is set up, so Ghost types what it heard.");
    for (const tidy_source of ["off", "subscription", "openrouter"] as const)
      for (const openrouter of [true, false])
        for (const tidy_with of [null, "Claude Code", "OpenRouter", "Anthropic"]) expect(tidyWhere({ tidy_source, tidy_with, openrouter }).toLowerCase()).not.toContain("password");
    expect(rows({ ...ready, tidy_with: null, tidy_model: "", subscription: false })).toContain("No model is set up");
  });

  it("offers Off, your Claude Code's three and OpenRouter's, which wait for its key", () => {
    const flat = (v: Parameters<typeof tidyOptions>[0]) => tidyOptions(v).flatMap((g) => g.options.map((o) => ({ ...o, group: g.label })));
    const opts = flat(ready);
    expect(opts.map((o) => o.value)).toEqual([
      "off",
      "subscription:haiku",
      "subscription:sonnet",
      "subscription:opus",
      ...OPENROUTER_MODELS.map((m) => `openrouter:${m.id}`),
      ANOTHER_OPENROUTER,
    ]);
    expect(opts.find((o) => o.value === "subscription:haiku")?.label).toBe("Claude Code · Haiku");
    expect(opts.find((o) => o.value === "subscription:haiku")?.group).toBe("Your Claude Code, about 3 s");
    // No key: OpenRouter's are there, disabled; with it, enabled.
    expect(opts.filter((o) => o.value.startsWith("openrouter")).every((o) => o.disabled)).toBe(true);
    expect(flat({ ...ready, openrouter: true }).filter((o) => o.value.startsWith("openrouter")).every((o) => !o.disabled)).toBe(true);
    expect(tidyOptions({ ...ready, openrouter: true }).at(-1)?.label).toBe("OpenRouter, under a second");
    // Without Claude Code on this computer, its options aren't offered.
    const none = flat({ ...ready, subscription: false, tidy_source: "off", tidy_model: "" });
    expect(none.some((o) => o.value.startsWith("subscription"))).toBe(false);
    // A tidy kept from before, on Ghost's model of another kind, is shown as that.
    const kept = flat({ ...ready, subscription: false, tidy_model: "", tidy_with: "Anthropic" });
    expect(kept.find((o) => o.value === "subscription:")?.label).toBe("Ghost’s model (Anthropic)");
    // OpenRouter's ids are as OpenRouter writes them.
    for (const m of OPENROUTER_MODELS) expect(m.id).toMatch(/^[a-z0-9.-]+\/[a-z0-9.:-]+$/);
  });

  it("reads the daemon's choice into the select and sends a choice back", () => {
    expect(tidyValue(ready)).toBe("subscription:haiku");
    expect(tidyValue({ tidy_source: "off", tidy_model: "" })).toBe("off");
    expect(tidyValue({ tidy_source: "openrouter", tidy_model: "anthropic/claude-haiku-5.5" })).toBe("openrouter:anthropic/claude-haiku-5.5");
    expect(tidyValue({ tidy_source: "openrouter", tidy_model: "mistralai/ministral-8b" })).toBe(ANOTHER_OPENROUTER);
    expect(tidyBody("off")).toEqual({ tidy_source: "off" });
    expect(tidyBody("subscription:sonnet")).toEqual({ tidy_source: "subscription", tidy_model: "sonnet" });
    expect(tidyBody("openrouter:meta-llama/llama-3.1-8b-instruct:nitro")).toEqual({ tidy_source: "openrouter", tidy_model: "meta-llama/llama-3.1-8b-instruct:nitro" });
    expect(tidyBody(ANOTHER_OPENROUTER, " mistralai/ministral-8b ")).toEqual({ tidy_source: "openrouter", tidy_model: "mistralai/ministral-8b" });
  });

  it("draws the select, the typed id only for another model, and the key control while OpenRouter needs it", () => {
    const html = rows(ready, null, "OPENROUTER KEY ROW");
    expect(html).toContain("Tidy with");
    expect(html).toContain("<optgroup");
    expect(html).toContain("Claude Code · Sonnet");
    expect(html).not.toContain("Model on OpenRouter");
    expect(html).toContain("OPENROUTER KEY ROW"); // no key yet: right there
    // Key set and Claude Code chosen: the key row stays out of the way.
    expect(rows({ ...ready, openrouter: true }, null, "OPENROUTER KEY ROW")).not.toContain("OPENROUTER KEY ROW");
    const another = rows({ ...ready, openrouter: true, tidy_source: "openrouter", tidy_with: "OpenRouter", tidy_model: "mistralai/ministral-8b" }, null, "OPENROUTER KEY ROW");
    expect(another).toContain("Model on OpenRouter");
    expect(another).toContain("mistralai/ministral-8b");
    expect(another).toContain("OPENROUTER KEY ROW");
  });

  it("has a line for each state that needs one, in Ghost's words", () => {
    expect(keyNotes(ready)).toEqual([]);
    const unavailable = keyNotes({ on: false, state: "unavailable", typing: "unknown" });
    expect(unavailable[0].text).toBe("This desktop doesn’t let apps use a key this way. Make a keyboard shortcut of your own that runs ghostd dictate toggle; the text then goes to your clipboard.");
    expect(rows({ ...ready, on: false, state: "unavailable", key: null })).toContain("<code");
    expect(keyNotes({ on: true, state: "refused", typing: "unknown" })[0].text).toBe("GNOME said no to the key. Turn the switch off and on to be asked again.");
    expect(keyNotes({ on: true, state: "failed", typing: "unknown" })[0].text).toBe("Ghost couldn’t ask GNOME for the key.");
    expect(keyNotes({ ...ready, typing: "refused" })[0].text).toBe("Ghost isn’t allowed to type, so the text goes to your clipboard. Press Ctrl+V to paste it.");
    // Never the plumbing's words.
    for (const state of ["off", "binding", "ready", "unavailable", "refused", "failed"] as const)
      for (const typing of ["unknown", "allowed", "refused"] as const) {
        const html = rows({ ...ready, state, typing }).toLowerCase();
        for (const word of ["mcp", "server", "portal"]) expect(html).not.toContain(word);
      }
  });

  it("calls the key's routes: read, switch and tidy, and GNOME's picker", async () => {
    const calls: { method: string; path: string; body: string }[] = [];
    const fetch = (async (input: Request | string, init?: RequestInit) => {
      const req = input instanceof Request ? input : new Request(String(input), init);
      calls.push({ method: req.method, path: new URL(req.url).pathname, body: req.method === "GET" ? "" : await req.text() });
      return new Response(JSON.stringify(ready), { status: 200, headers: { "content-type": "application/json" } });
    }) as typeof globalThis.fetch;
    const client = new GhostClient({ baseUrl: "http://ghost", fetch, events: { subscribe: () => () => undefined } });
    await client.dictationKey();
    await client.setDictationKey({ on: true });
    await client.setDictationKey({ tidy: false });
    await client.setDictationTidy("subscription", "sonnet");
    await client.changeDictationKey();
    expect(calls.map((c) => [c.method, c.path, c.body])).toEqual([
      ["GET", "/dictation/key", ""],
      ["PUT", "/dictation/key", '{"on":true}'],
      ["PUT", "/dictation/key", '{"tidy":false}'],
      ["PUT", "/dictation/key", '{"tidy_source":"subscription","tidy_model":"sonnet"}'],
      ["POST", "/dictation/key/change", ""],
    ]);
  });

  it("turns on in the mock through GNOME's wait, and off at once", async () => {
    vi.useFakeTimers();
    try {
      const s = scenarioByName("passive");
      const d = new MockDaemon(s, composeFixtures(s), {});
      const q = new URLSearchParams();
      expect(((await d.handle("GET", "/dictation/key", q, null)) as DictationKeyView).state).toBe("off");
      const on = d.handle("PUT", "/dictation/key", q, { on: true }) as Promise<DictationKeyView>;
      await vi.advanceTimersByTimeAsync(3_000);
      expect(await on).toMatchObject({ on: true, state: "ready", key: "Press F9" });
      const off = (await d.handle("PUT", "/dictation/key", q, { on: false })) as DictationKeyView;
      expect(off).toMatchObject({ on: false, state: "off", key: null });
      d.dispose();
    } finally {
      vi.useRealTimers();
    }
    expect(keyViewFor(new URLSearchParams("dictkey=unavailable")).on).toBe(false);
    expect(keyViewFor(new URLSearchParams("dictkey=ready&typing=refused&tidy=none"))).toMatchObject({ key: "Press F9", typing: "refused", tidy_with: null, subscription: false });
    expect(keyViewFor(new URLSearchParams(""))).toMatchObject({ tidy_source: "subscription", tidy_model: "haiku", tidy_with: "Claude Code" });
    expect(keyViewFor(new URLSearchParams("tidy=another"))).toMatchObject({ tidy_source: "openrouter", openrouter: true });
  });

  it("follows Tidy with in the mock, OpenRouter's key from the vault", async () => {
    const s = scenarioByName("passive");
    const d = new MockDaemon(s, composeFixtures(s), {});
    const q = new URLSearchParams();
    const set = (b: object) => d.handle("PUT", "/dictation/key", q, b) as Promise<DictationKeyView>;
    expect(await set({ tidy_source: "subscription", tidy_model: "opus" })).toMatchObject({ tidy_source: "subscription", tidy_model: "opus", tidy_with: "Claude Code" });
    const or = await set({ tidy_source: "openrouter", tidy_model: "google/gemini-3.5-flash-lite" });
    expect(or).toMatchObject({ tidy_source: "openrouter", tidy_model: "google/gemini-3.5-flash-lite" });
    expect(or.tidy_with).toBe(or.openrouter ? "OpenRouter" : null);
    expect(await set({ tidy_source: "off" })).toMatchObject({ tidy: false, tidy_source: "off", tidy_with: null });
    d.dispose();
  });

  it("plays each designed dictation as the daemon sends it, ending the way its scenario says", async () => {
    vi.useFakeTimers();
    try {
      for (const [name, end] of [["dictation-typed", "typed"], ["dictation-clipboard", "clipboard"], ["dictation-nothing", "nothing_heard"], ["dictation-stopped", "stopped"]] as const) {
        const s = scenarioByName(name);
        expect(s.name).toBe(name);
        const d = new MockDaemon(s, composeFixtures(s), {});
        const seen: DictationLiveView[] = [];
        d.subscribe((e) => {
          if ("type" in e && (e as GhostEvent).type === "dictation") seen.push((e as Extract<GhostEvent, { type: "dictation" }>).dictation);
        });
        await vi.advanceTimersByTimeAsync(9_000);
        d.dispose();
        const scenes = seen.map((v) => v.scene);
        expect(scenes[0], name).toBe("starting");
        expect(scenes, name).toContain("listening");
        expect(scenes, name).toContain(end);
        if (end !== "nothing_heard") expect(seen.some((v) => !!v.text), name).toBe(true);
      }
    } finally {
      vi.useRealTimers();
    }
  });
});

describe("the words in the glow", () => {
  it("fades in only the phrase just read, and dims what was already shown", () => {
    expect(splitWords(null, "First part.")).toEqual({ older: "", newest: "First part." });
    expect(splitWords("First part.", "First part. Second part.")).toEqual({ older: "First part.", newest: "Second part." });
  });

  it("shows all of it as new when the text doesn't continue the last", () => {
    expect(splitWords("Something else", "First part.")).toEqual({ older: "", newest: "First part." });
    expect(splitWords("Same", "Same")).toEqual({ older: "", newest: "Same" });
  });

  it("cuts the older words from the front to keep two lines", () => {
    const before = "one two three four five six seven eight nine ten eleven twelve thirteen fourteen";
    const { older, newest } = splitWords(before, `${before} and the newest phrase`, 50);
    expect(newest).toBe("and the newest phrase");
    expect(older.startsWith("…")).toBe(true);
    expect(older.length + newest.length).toBeLessThanOrEqual(51);
    expect(before.endsWith(older.slice(1))).toBe(true);
  });
});
