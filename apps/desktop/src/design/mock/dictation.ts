/**
 * The mock's dictation (ADR 0077 as amended 2026-11-25): the key in Settings › Sound, and a
 * dictation's live view for the floating surface and Today's live strip. Synthetic words only.
 *
 *   ?dictkey=off|ready|unavailable|refused|failed   where the key stands (default off; `ready`
 *                    is F9, bound)
 *   &typing=refused  Ghost isn't allowed to type: the text goes to the clipboard
 *   &tidy=none       no model set up, so nothing to tidy with
 *   &tidy=openrouter OPENROUTER_API_KEY set: the tidy goes to OpenRouter (ADR 0078)
 *
 * Scenarios (`?mock=dictation…`, on `floating.html` or the main window): `dictation` plays every
 * ending in turn, over and over; `dictation-listening` and `dictation-writing` hold that state;
 * `dictation-typed`, `-clipboard`, `-nothing` and `-stopped` play a short dictation that ends
 * that way, every few seconds.
 */
import type { DictationKeyBody, DictationKeyView, DictationLiveView, DictationScene, GhostEvent } from "@ghost/client-ts";

export type DictationDesign = "loop" | "listening" | "writing" | "typed" | "clipboard" | "nothing_heard" | "stopped";

const WORDS = "Let's move the design review to Thursday afternoon and ask Dana to bring the new onboarding numbers, then send the notes to the whole team before Friday.";
const SENTENCES: Partial<Record<DictationScene, string>> = {
  typed: "Typed 152 characters.",
  clipboard: "Some characters can't be typed here. On your clipboard: 152 characters. Press Ctrl+V.",
  nothing_heard: "Nothing heard.",
  stopped: "Stopped: a session opened.",
};

const LIMIT = 120;

export function keyViewFor(params: URLSearchParams): DictationKeyView {
  const k = params.get("dictkey");
  const state = k === "ready" || k === "unavailable" || k === "refused" || k === "failed" ? k : "off";
  return {
    on: state !== "off" && state !== "unavailable",
    state,
    key: state === "ready" ? "Press F9" : null,
    typing: params.get("typing") === "refused" ? "refused" : state === "ready" ? "allowed" : "unknown",
    tidy: true,
    tidy_with: params.get("tidy") === "none" ? null : params.get("tidy") === "openrouter" ? "OpenRouter" : "Anthropic",
    tidy_model: "anthropic/claude-haiku-5.5",
    tidy_model_default: "anthropic/claude-haiku-5.5",
    openrouter: params.get("tidy") === "openrouter",
  };
}

export class MockDictation {
  private key: DictationKeyView;
  private timers = new Set<ReturnType<typeof setTimeout>>();
  private played = false;

  constructor(
    private readonly emit: (e: GhostEvent) => void,
    params: URLSearchParams,
    private readonly design: DictationDesign | null,
  ) {
    this.key = keyViewFor(params);
  }

  handle(method: string, path: string, body: Record<string, unknown>): Promise<DictationKeyView> | DictationKeyView | undefined {
    if (method === "GET" && path === "/dictation/key") return this.key;
    if (method === "PUT" && path === "/dictation/key") return this.set(body as DictationKeyBody);
    if (method === "POST" && path === "/dictation/key/change") return this.wait(900).then(() => (this.key = { ...this.key, key: this.key.key === "Press F9" ? "Press Ctrl+Alt+D" : "Press F9" }));
    return undefined;
  }

  private async set(b: DictationKeyBody): Promise<DictationKeyView> {
    if (b.tidy != null) this.key = { ...this.key, tidy: b.tidy };
    if (b.tidy_model != null) this.key = { ...this.key, tidy_model: b.tidy_model.trim() || (this.key.tidy_model_default ?? "") };
    if (b.on === false) this.key = { ...this.key, on: false, state: "off", key: null };
    if (b.on === true && !this.key.on) {
      // GNOME's two dialogs: the key, then typing. The switch waits for both.
      this.key = { ...this.key, on: true, state: "binding" };
      await this.wait(2600);
      this.key = { ...this.key, state: "ready", key: "Press F9", typing: this.key.typing === "refused" ? "refused" : "allowed" };
    }
    return this.key;
  }

  /** Start the scenario's dictation on the first subscriber. */
  play() {
    if (!this.design || this.played) return;
    this.played = true;
    const endings: DictationScene[] = ["typed", "clipboard", "nothing_heard", "stopped"];
    let round = 0;
    const loop = () => {
      const end: DictationScene | null =
        this.design === "loop" ? endings[round++ % endings.length] : this.design === "listening" || this.design === "writing" ? null : (this.design as DictationScene);
      const total = this.dictate(end, this.design === "writing");
      if (total != null) this.after(total + 1500, loop);
    };
    this.after(600, loop);
  }

  /** One dictation that ends as `end` (null: held listening, or held writing). Its length in ms, or null when held. */
  private dictate(end: DictationScene | null, holdWriting: boolean): number | null {
    const key = true;
    const view = (scene: DictationScene, o: Partial<DictationLiveView> = {}): DictationLiveView => ({
      scene,
      key,
      level: null,
      text: null,
      seconds: 0,
      limit_secs: LIMIT,
      sentence: null,
      ...o,
    });
    const send = (v: DictationLiveView) => this.emit({ type: "dictation", dictation: v });
    send(view("starting"));
    const heard = end === "nothing_heard" ? "" : WORDS;
    const words = heard.split(" ");
    const listenMs = end === "nothing_heard" ? 1800 : end === null && !holdWriting ? Number.POSITIVE_INFINITY : 4200;
    let t = 300;
    const startAt = t;
    // As the daemon sends it: the level fifteen a second with the words read so far, which
    // grow a piece at a time at the speaker's pauses (every 0.6 s here).
    const tick = (at: number) => {
      if (at - startAt > listenMs) return;
      this.after(at, () => {
        const s = (at - startAt) / 1000;
        const shown = Math.min(words.length, Math.floor(Math.floor(s / 0.6) * 0.6 * 5.5));
        const level = end === "nothing_heard" ? 0.04 + 0.03 * Math.random() : 0.25 + 0.55 * Math.abs(Math.sin(s * 7.3)) * Math.random();
        send(view("listening", { level, seconds: s, text: shown > 0 ? words.slice(0, shown).join(" ") : null }));
        tick(at + 66);
      });
    };
    tick(t);
    if (!Number.isFinite(listenMs)) return null;
    t += listenMs;
    const text = heard || null;
    this.after(t, () => send(view("writing", { text, seconds: listenMs / 1000 })));
    if (holdWriting) return null;
    t += end === "nothing_heard" ? 400 : 900;
    const scene = end ?? "typed";
    this.after(t, () => send(view(scene, { text, seconds: listenMs / 1000, sentence: SENTENCES[scene] ?? null })));
    return t + (scene === "typed" || scene === "nothing_heard" ? 1000 : 3000);
  }

  private after(ms: number, f: () => void) {
    const id = setTimeout(() => {
      this.timers.delete(id);
      f();
    }, ms);
    this.timers.add(id);
  }

  private wait(ms: number): Promise<void> {
    return new Promise((r) => this.after(ms, r));
  }

  dispose() {
    for (const t of this.timers) clearTimeout(t);
    this.timers.clear();
  }
}
