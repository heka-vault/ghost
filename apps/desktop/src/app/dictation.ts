/**
 * A dictation as the desktop shows it (ADR 0077 as amended 2026-11-25): on the floating surface
 * when the posture floats (`postureOf`'s `dictation`), else in Today's live strip. The state in
 * a few words, the microphone's level, the words read so far (the one place dictated words are
 * shown: beside the work, never kept), then Ghost's own sentence for how it ended, for a moment.
 *
 * The words and the timing are pure functions here, tested; `useDictation` is the one hook both
 * surfaces read.
 */
import type { DictationLiveView, DictationScene } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

/** The scenes that say how a dictation ended. */
export const ENDED: readonly DictationScene[] = ["typed", "clipboard", "nothing_heard", "stopped"];

export function isEnded(scene: DictationScene): boolean {
  return ENDED.includes(scene);
}

/**
 * How long the end stays on screen before the scene goes: a moment after it typed or heard
 * nothing (nothing to do), longer when the person has something to do (paste) or to read (why
 * it stopped). Null while the dictation is still going on.
 */
export function lingerMs(scene: DictationScene): number | null {
  switch (scene) {
    case "typed":
    case "nothing_heard":
      return 1_000;
    case "clipboard":
    case "stopped":
      return 3_000;
    default:
      return null;
  }
}

/** The colour the state is drawn in: listening (green), writing (violet), done (cyan). */
export type DictationTone = "listening" | "writing" | "done";

export function toneOf(scene: DictationScene): DictationTone {
  if (scene === "starting" || scene === "listening") return "listening";
  if (scene === "writing") return "writing";
  return "done";
}

/**
 * The state in a few words: “Listening” while it hears, “Writing it down” while it reads and
 * types, then how it ended (“Typed”, “On your clipboard”, “Nothing heard”, “Stopped”). Short
 * enough for the floating label; Ghost's own sentence goes under it (`dictationSentence`).
 */
export function dictationTitle(v: Pick<DictationLiveView, "scene">): string {
  switch (v.scene) {
    case "starting":
    case "listening":
      return "Listening";
    case "writing":
      return "Writing it down";
    case "typed":
      return "Typed";
    case "clipboard":
      return "On your clipboard";
    case "nothing_heard":
      return "Nothing heard";
    case "stopped":
      return "Stopped";
  }
}

/**
 * Ghost's own sentence for how it ended, when it says more than the title: what to do (“… On
 * your clipboard: 12 characters. Press Ctrl+V.”) or why it stopped. Null while it goes on, after
 * it typed (the text is where the cursor was: “Typed” is all), and when it only repeats the title.
 */
export function dictationSentence(v: Pick<DictationLiveView, "scene" | "sentence">): string | null {
  if (!isEnded(v.scene) || v.scene === "typed") return null;
  const said = v.sentence?.trim() || null;
  if (v.scene === "clipboard") return said ?? "Press Ctrl+V to paste it.";
  if (!said) return null;
  const plain = (x: string) => x.toLowerCase().replace(/[^a-z]/g, "");
  return plain(said) === plain(dictationTitle(v)) ? null : said;
}

/**
 * The words so far as they fit on screen: the newest kept, cut at a word, with “…” before. Never
 * kept anywhere: this is the drawing's string, made again from the event each time.
 */
export function wordsShown(text: string | null | undefined, max = 160): string {
  const said = (text ?? "").replace(/\s+/g, " ").trim();
  if (said.length <= max) return said;
  const tail = said.slice(said.length - max);
  const space = tail.indexOf(" ");
  return `…${space >= 0 ? tail.slice(space + 1) : tail}`;
}

/**
 * The words so far, split for the floating room: what was already shown (`older`, dimmed and
 * cut from the front to fit) and the phrase just read (`newest`, which fades in). `before` is
 * the text the last time; when the new text doesn't continue it, all of it is new.
 */
export function splitWords(before: string | null | undefined, text: string | null | undefined, max = 120): { older: string; newest: string } {
  const said = (text ?? "").replace(/\s+/g, " ").trim();
  const prev = (before ?? "").replace(/\s+/g, " ").trim();
  const continues = prev.length > 0 && said.startsWith(prev) && said.length > prev.length;
  const newest = continues ? said.slice(prev.length).trim() : said;
  const older = continues ? prev : "";
  const room = Math.max(0, max - newest.length - 1);
  return { older: older ? wordsShown(older, room) : "", newest };
}

/** A dictation as the page holds it: the last view, and when its end arrived (performance.now()). */
export interface DictationSeen {
  view: DictationLiveView;
  endedAt: number | null;
}

/** The next held state from a new view: the end's clock starts at the first end scene seen. */
export function nextSeen(prev: DictationSeen | null, view: DictationLiveView | null, now: number): DictationSeen | null {
  if (!view) return null;
  if (!isEnded(view.scene)) return { view, endedAt: null };
  const sameEnd = prev && prev.endedAt != null && prev.view.scene === view.scene;
  return { view, endedAt: sameEnd ? prev.endedAt : now };
}

/** When the scene goes (performance.now()), or null while the dictation is still going on. */
export function goneAt(seen: DictationSeen | null): number | null {
  if (!seen || seen.endedAt == null) return null;
  return seen.endedAt + (lingerMs(seen.view.scene) ?? 0);
}

/** Whether the dictation is on screen now: going on, or its end still within its moment. */
export function dictationShown(seen: DictationSeen | null, now: number): boolean {
  if (!seen) return false;
  const gone = goneAt(seen);
  return gone == null || now < gone;
}

const clock = () => (typeof performance !== "undefined" ? performance.now() : Date.now());

/**
 * The dictation now, for a surface to draw: the view (null when there is none to show), whether
 * it is shown, and the words for its state. Re-renders on a change of scene, words or sentence,
 * never on the level (read `useGhost().dictation.level()` from a frame, or subscribe).
 *
 * The end goes after its moment by a timer and by `check(now)`, which the floating surface
 * calls from its frame loop: WebKit throttles a page's timers in a window that never has focus.
 */
export function useDictation(): { view: DictationLiveView | null; shown: boolean; title: string; sentence: string | null; tone: DictationTone; check: (now: number) => void } {
  const { dictation } = useGhost();
  // A view already there when this mounts: shown if it is still going on, never an old end again.
  const [seen, setSeen] = useState<DictationSeen | null>(() => {
    const v = dictation.get();
    return v && !isEnded(v.scene) ? nextSeen(null, v, clock()) : null;
  });
  const [, bump] = useState(0);
  useEffect(() => dictation.subscribe(() => setSeen((prev) => nextSeen(prev, dictation.get(), clock()))), [dictation]);
  const gone = goneAt(seen);
  useEffect(() => {
    if (gone == null) return;
    const t = setTimeout(() => bump((n) => n + 1), Math.max(0, gone - clock()) + 20);
    return () => clearTimeout(t);
  }, [gone]);
  const shown = dictationShown(seen, clock());
  const view = shown ? (seen?.view ?? null) : null;
  return {
    view,
    shown,
    title: view ? dictationTitle(view) : "",
    sentence: view ? dictationSentence(view) : null,
    tone: view ? toneOf(view.scene) : "listening",
    check: (now: number) => {
      if (gone != null && now >= gone && shown) bump((n) => n + 1);
    },
  };
}
