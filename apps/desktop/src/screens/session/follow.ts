/**
 * Whether the conversation follows its newest turn (#90's bug: "I keep seeing New reply").
 *
 * The thread stays at its end while the operator is there, and lets go only when they scroll
 * up themselves: a wheel or touch moving up, a key (PageUp, ArrowUp, Home, Shift+Space) or a
 * drag on the scrollbar, then a scroll up within `INTENT_MS`. A scroll up that nobody asked
 * for never lets go: the room view scrolling the panel to its top, the browser clamping or
 * anchoring the scroll as content above shrinks, a smooth scroll cut short. Reaching the end
 * pins again; a new session, the operator's own words and a change waiting for their okay
 * always follow. Pure, so it is tested without a browser.
 */

/** How long after the operator's input a scroll up counts as theirs. */
export const INTENT_MS = 600;
/** Within this of the end, the thread counts as followed. */
export const PINNED_PX = 64;

export interface ScrollState {
  top: number;
  height: number;
  client: number;
}

export class Follow {
  pinned = true;
  private lastTop = 0;
  private intentAt = Number.NEGATIVE_INFINITY;

  /** The operator reached for the scroll (wheel or touch up, a key, the scrollbar). */
  intent(now: number) {
    this.intentAt = now;
  }

  /** The panel scrolled, by anyone. */
  scrolled(s: ScrollState, now: number) {
    const near = s.height - s.top - s.client < PINNED_PX;
    const up = s.top < this.lastTop;
    this.lastTop = s.top;
    if (near) this.pinned = true;
    else if (up && now - this.intentAt <= INTENT_MS) this.pinned = false;
  }

  /** Something new arrived: go to the end, or mark it for an operator reading above. */
  onNew(why: { sent?: boolean; asked?: boolean } = {}): "follow" | "mark" {
    if (why.sent || why.asked) this.pinned = true;
    return this.pinned ? "follow" : "mark";
  }

  /** A session started: it is followed from its first turn. */
  newSession() {
    this.pinned = true;
  }
}

/** Keys that scroll a page up. */
export function scrollsUp(e: KeyboardEvent): boolean {
  return e.key === "PageUp" || e.key === "ArrowUp" || e.key === "Home" || (e.key === " " && e.shiftKey);
}
