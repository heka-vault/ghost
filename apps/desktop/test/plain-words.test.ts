/**
 * No variable names, device ids, tool ids or repo paths on the pages everyone sees (the flow
 * review, 2026-11-14: Settings › Sound showed six `GHOST_*` names, `ghost.mic` five times and
 * `hw:0,0`; General a monitor's connector and `docs/floating-room.md`; the catalog's cards a
 * hostname). They belong on the developer page or behind a row's Details.
 *
 * The tests run in Node with no DOM, so this reads what the everyday pages render from the mock's
 * own fixtures: the On its own list drawn whole (renderToStaticMarkup) from the busy scenario, and
 * the words the Settings, Apps and first-run pages build from the same data (the microphone's
 * choices and its sentence, how Ghost reaches you, where an app runs). Text inside
 * `data-user-content` (someone's own words) and inside a closed disclosure is left out, as on
 * screen. A last pass reads the everyday pages' own source for a literal that would show one.
 */
import { readFileSync } from "node:fs";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import { describe, expect, it, vi } from "vitest";

// settings/model.ts reads the address at load (its draft's key); Node has no window.
vi.hoisted(() => {
  const g = globalThis as { window?: unknown };
  g.window ??= { location: { search: "", hash: "" } };
});

import { liveRow, rowOf, STAGES } from "@ghost/ui";

import { CATALOG } from "../src/design/mock/fixtures/apps";
import { seededTimings } from "../src/design/mock/live";
import { schedules, watching } from "../src/design/mock/fixtures/folder";
import { ROUTES } from "../src/design/mock/fixtures/policy";
import settingsFixture from "../src/design/mock/fixtures/settings";
import { scenarioByName } from "../src/design/mock/scenarios";
import { placesOf, placeWords } from "../src/screens/apps/provenance";
import { inputName } from "../src/screens/onboarding/state";
import { micInUse, micOptions, wakeMicOptions } from "../src/screens/settings/model";
import { deviceWords, plainDeviceText, reachSentence } from "../src/screens/settings/plain";
import { OnItsOwnList } from "../src/screens/watching/List";
import { problemOf } from "../src/screens/watching/problems";
import { doingsOf, placeProblems, sentenceOf } from "../src/screens/watching/sentence";

/** What may never show on an everyday page, outside the developer page and Details. */
export const FORBIDDEN: { what: string; re: RegExp }[] = [
  { what: "an environment variable", re: /\bGHOST_[A-Z_]+\b/ },
  { what: "a device id", re: /\bhw:\d/ },
  { what: "Ghost's echo-cancelled device's id", re: /\bghost\.mic\b/ },
  { what: "a dotted tool id", re: /\b[a-z][a-z0-9]*_[a-z0-9_]+\.[a-z][a-z0-9_]*\b|\b[a-z][a-z0-9_]*\.[a-z][a-z0-9]*_[a-z0-9_]+\b/ },
  { what: "a repo path", re: /(?<![\w@/-])(docs|crates|apps|tools|infra)\/[\w./-]+/ },
  { what: "a 24-hour time in an en-US sentence", re: /\b(1[3-9]|2[0-3]):[0-5]\d\b/ },
  { what: "a hostname", re: /\b[a-z0-9-]+\.[a-z0-9-]+\.(com|app|io|dev|net|org)\b/ },
];

const offenders = (text: string) => FORBIDDEN.filter((f) => f.re.test(text)).map((f) => `${f.what} in “${text.match(f.re)?.[0]}”`);

/** The visible text of static markup: tags out, someone's own words and closed disclosures out. */
function visibleText(html: string): string {
  return html
    .replace(/<details(?![^>]*\sopen)[^>]*>[\s\S]*?<\/details>/g, " ")
    .replace(/<(\w+)[^>]*data-user-content[^>]*>[\s\S]*?<\/\1>/g, " ")
    .replace(/<[^>]+>/g, " ")
    .replace(/&amp;/g, "&")
    .replace(/&#x27;|&#39;/g, "'")
    .replace(/&quot;/g, '"')
    .replace(/\s+/g, " ");
}

const names: Record<string, string> = { ghost: "Ghost", accounting: "Accounting", ledger: "Ledger" };
const ctx = { zone: "America/Detroit", locale: "en-US", agentName: (s: string) => names[s] ?? s, appTitle: (n: string) => ({ github: "GitHub", calendar: "Calendar" })[n] ?? n };
const noop = () => undefined;

describe("the everyday pages say things in the operator's words", () => {
  it("draws On its own from the busy mock with no variable, id, path, password name or 24-hour time", () => {
    const busy = scenarioByName("busy");
    const w = watching(busy);
    const all = doingsOf(schedules(busy).view.schedules, w.watches, w.hooks);
    const problems = [...schedules(busy).view.problems, ...w.problems].map((raw) => problemOf(raw, ctx));
    const placed = placeProblems(all, problems);
    const html = renderToStaticMarkup(
      createElement(OnItsOwnList, {
        rows: all.map((d) => ({ d, s: sentenceOf(d, ctx), problem: placed.byKey.get(d.key) ?? null })),
        problems: placed.rows,
        busyKey: null,
        online: true,
        on: { toggle: noop, run: noop, stop: noop, edit: noop, history: noop, details: noop, remove: noop, problemDetails: noop },
      }),
    );
    const text = visibleText(html);
    expect(text).toContain("At a time");
    expect(text).toContain("When something happens");
    expect(offenders(text)).toEqual([]);
    // What the review found there, gone from the surface.
    for (const gone of ["Yours, in Ghost’s folder", "Checked for its signature", "deliveries", "Looks every", "STRIPE_HOOK_SECRET", "When something new turns up"]) expect(text).not.toContain(gone);
  });

  it("names every microphone the mock knows by the system's name, never its id", () => {
    for (const name of ["passive", "busy", "firstrun"]) {
      const f = settingsFixture(scenarioByName(name));
      const d = f.devices;
      const shown = [
        ...micOptions(d, d.input_device ? String(d.input_device) : "").map((o) => o.label),
        ...wakeMicOptions(d, "").map((o) => o.label),
        micInUse(d),
        deviceWords(d.output_device ?? null),
        d.microphone ? plainDeviceText(d.microphone.summary) : "",
        d.microphone ? deviceWords(inputName(d.microphone.wake_input)) : "",
      ];
      for (const t of shown) expect(offenders(t), `${name}: ${t}`).toEqual([]);
    }
    expect(deviceWords("Shure MV7+: USB Audio (hw:2,0)")).toBe("Shure MV7+");
    expect(deviceWords("HDA Intel PCH: ALC1220 Analog (hw:0,0)")).toBe("Built-in audio (ALC1220)");
    expect(deviceWords("pipewire#0")).toBe("The sound server (PipeWire), channel 1 alone");
    expect(deviceWords("ghost.mic")).toBe("Your microphone, without Ghost’s voice");
  });

  it("says how Ghost reaches you in the locale's clock, with no service's code name", () => {
    const said = reachSentence(ROUTES, "en-US");
    expect(offenders(said)).toEqual([]);
    expect(said).toMatch(/Quiet from 10 PM to 7 AM/);
    expect(said).not.toMatch(/ntfy/i);
  });

  it("says where a catalog app runs without its hostname or package", () => {
    for (const e of CATALOG) {
      for (const p of placesOf(e as never)) {
        const words = placeWords(p, e.maker?.product ?? e.product);
        expect(offenders(words), `${e.title}: ${words}`).toEqual([]);
        expect(words).not.toContain(p.where);
      }
    }
  });

  it("keeps Developer's timing overlay, which shows on Today and the conversation when switched on, to the same words", () => {
    // ADR 0073 as amended: numbers, stage labels and the model's name only.
    const rows = [...seededTimings("s1").map(rowOf), liveRow([{ kind: "you_stopped", session_id: "s1", at: new Date().toISOString() }], null)!];
    const heads = ["s", ...STAGES.map(([, l]) => l), "model"].join(" ");
    for (const r of rows) {
      const text = [heads, r.live ? "now" : `#${r.turn}`, ...STAGES.map(([k]) => r.cells[k]), r.model, r.cut ? "cut" : ""].join(" ");
      expect(offenders(text), text).toEqual([]);
    }
  });

  it("puts no variable name, device id or repo path in the everyday pages' own words", () => {
    // Literal strings only (what could reach the screen), comments left out.
    const files = [
      "src/screens/settings/General.tsx",
      "src/app/FloatingSettings.tsx",
      "src/screens/settings/ErrorReports.tsx",
      "src/screens/settings/RemoveGhost.tsx",
      "src/screens/settings/WakeTakesRow.tsx",
      "src/screens/settings/GhostVoice.tsx",
      "src/screens/watching/List.tsx",
      "src/screens/watching/sentence.ts",
      "src/screens/onboarding/frame.tsx",
    ];
    for (const f of files) {
      const src = readFileSync(new URL(`../${f}`, import.meta.url), "utf8").replace(/\/\*[\s\S]*?\*\//g, "").replace(/^\s*\/\/.*$/gm, "").replace(/^import[^;]*;/gm, "").replace(/import\(\s*"[^"]*"\s*\)/g, "");
      const literals = src.match(/"[^"\n]*"|`[^`]*`|>[^<>{}\n]+</g) ?? [];
      // A template's code (`${…}`) is code, not words.
      for (const l of literals) expect(offenders(l.replace(/\$\{[^}]*\}/g, "")), `${f}: ${l}`).toEqual([]);
    }
    // Settings › Sound's fields name no variable beside their label any more.
    const sound = readFileSync(new URL("../src/screens/settings/index.tsx", import.meta.url), "utf8");
    expect(sound).not.toMatch(/detail=\{?"?(GHOST_|FUZZY)/);
    const phrases = readFileSync(new URL("../src/screens/settings/WakePhrases.tsx", import.meta.url), "utf8");
    expect(phrases).not.toMatch(/detail=\{WAKE_KEY\}/);
  });
});
