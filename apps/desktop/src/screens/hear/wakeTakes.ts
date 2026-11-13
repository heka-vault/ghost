/**
 * Kept wake takes (ADR 0065): the labelling's rules, apart from the screen so they are
 * tested on their own. A take is labelled with one key and the next unlabelled one plays at
 * once, so a day's takes are labelled in a couple of minutes.
 *
 *   Y   me calling Ghost          N   not a wake
 *   B   me, and I had called just before and it missed
 *   U   clear the label           Space  play again    A  play the array's own stream
 *   ↓/J next   ↑/K previous      Delete/X  delete this take
 */
import type { WakeTakeLabel, WakeTakeView } from "@ghost/client-ts";

/** The one line said wherever a person will see it (Settings › Sound, the first run). */
export const KEEP_NOTICE =
  "Ghost keeps the sound of each time it thinks it heard its name, on this computer, to learn your voice.";

/** Said instead under transcript wake, which keeps nothing it hears before its name. */
export const NOTHING_KEPT = "Ghost keeps nothing it hears before its name.";

/** The wake that keeps takes: the trained detector (`GHOST_WAKE_MODE=onnx`). */
export const DETECTOR_WAKE = "onnx";

export interface Keeping {
  /** The line to say: the keeping notice under the detector, else the honest one. */
  notice: string;
  /** The switch and the labelling: only while takes are being kept. */
  controls: boolean;
  /** Settings › Sound shows a Wake takes row: under the detector, or takes left from it. */
  row: boolean;
}

/**
 * What to say about wake takes for the wake Ghost is using (`Status.wake_mode`). Takes are
 * kept only under the trained detector; transcript wake keeps none, so it says nothing
 * about takes, except that ones kept earlier are still there to delete.
 */
export function keeping(wakeMode: string | null | undefined, count: number): Keeping {
  if (wakeMode === DETECTOR_WAKE) return { notice: KEEP_NOTICE, controls: true, row: true };
  if (count > 0) {
    return {
      notice: `${NOTHING_KEPT} These were kept while the trained detector was the wake; delete them when you like.`,
      controls: false,
      row: true,
    };
  }
  return { notice: NOTHING_KEPT, controls: false, row: false };
}

export type TakeAction =
  | { kind: "label"; label: WakeTakeLabel | null }
  | { kind: "play"; array: boolean }
  | { kind: "move"; by: 1 | -1 }
  | { kind: "delete" };

export interface KeyLike {
  key: string;
  ctrlKey?: boolean;
  metaKey?: boolean;
  altKey?: boolean;
}

/** What a key does on the list; null for anything else (and any key with a modifier). */
export function keyAction(e: KeyLike): TakeAction | null {
  if (e.ctrlKey || e.metaKey || e.altKey) return null;
  switch (e.key) {
    case "y":
    case "Y":
      return { kind: "label", label: "me" };
    case "n":
    case "N":
      return { kind: "label", label: "not" };
    case "b":
    case "B":
      return { kind: "label", label: "me_missed_before" };
    case "u":
    case "U":
      return { kind: "label", label: null };
    case " ":
      return { kind: "play", array: false };
    case "a":
    case "A":
      return { kind: "play", array: true };
    case "ArrowDown":
    case "j":
    case "J":
      return { kind: "move", by: 1 };
    case "ArrowUp":
    case "k":
    case "K":
      return { kind: "move", by: -1 };
    case "Delete":
    case "Backspace":
    case "x":
    case "X":
      return { kind: "delete" };
    default:
      return null;
  }
}

/** The take to go to after `from` was labelled: the next unlabelled one after it, else the
 *  first unlabelled one before it, else the one after it, else -1 for none. */
export function nextAfterLabel(takes: WakeTakeView[], from: number): number {
  for (let i = from + 1; i < takes.length; i++) if (!takes[i].label) return i;
  for (let i = 0; i < from; i++) if (!takes[i].label) return i;
  if (from + 1 < takes.length) return from + 1;
  return takes.length ? Math.min(from, takes.length - 1) : -1;
}

/** Where the selection goes after the take at `from` is deleted from a list of `n`. */
export function afterDelete(n: number, from: number): number {
  if (n <= 1) return -1;
  return Math.min(from, n - 2);
}

export function move(n: number, from: number, by: 1 | -1): number {
  if (n === 0) return -1;
  return Math.max(0, Math.min(n - 1, from + by));
}

export interface Tally {
  me: number;
  missedBefore: number;
  not: number;
  unlabelled: number;
  /** What the retraining will learn from: positives and negatives. */
  positives: number;
  negatives: number;
}

/** What the retraining will use (`wake_train.data.label_of`): the operator's label; an
 *  accepted wake with no label as theirs only when they said so; never an unlabelled
 *  refused hit. */
export function tally(takes: WakeTakeView[], unlabelledAcceptedAreMe: boolean): Tally {
  const t: Tally = { me: 0, missedBefore: 0, not: 0, unlabelled: 0, positives: 0, negatives: 0 };
  for (const k of takes) {
    if (k.label === "me") t.me++;
    else if (k.label === "me_missed_before") t.missedBefore++;
    else if (k.label === "not") t.not++;
    else t.unlabelled++;
    if (k.label === "me" || k.label === "me_missed_before") t.positives++;
    else if (k.label === "not") t.negatives++;
    else if (k.outcome === "accepted" && unlabelledAcceptedAreMe) t.positives++;
  }
  return t;
}

export function bytesText(n: number): string {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${Math.round(n / 1024)} KB`;
  if (n < 1024 * 1024 * 1024) return `${(n / (1024 * 1024)).toFixed(n < 10 * 1024 * 1024 ? 1 : 0)} MB`;
  return `${(n / (1024 * 1024 * 1024)).toFixed(1)} GB`;
}

export const LABEL_TEXT: Record<WakeTakeLabel, string> = {
  me: "Me calling Ghost",
  not: "Not a wake",
  me_missed_before: "Me, after a miss",
};

export function outcomeText(t: WakeTakeView): string {
  return t.outcome === "accepted" ? "Woke" : "Turned down";
}

/** The score curve as an SVG path in a `w`×`h` box, 0 at the bottom and 1 at the top. */
export function curvePath(scores: number[], w: number, h: number): string {
  if (scores.length < 2) return "";
  const dx = w / (scores.length - 1);
  return scores
    .map((s, i) => `${i ? "L" : "M"}${(i * dx).toFixed(1)} ${(h - Math.max(0, Math.min(1, s)) * h).toFixed(1)}`)
    .join(" ");
}
