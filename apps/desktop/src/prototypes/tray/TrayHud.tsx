/**
 * Owner: tray. The on-screen HUD: a small capsule at the bottom center of the screen while
 * Ghost is awake, so the operator knows the microphone is open without opening anything.
 * Nothing shows while Ghost is passive: idle audio stays on the machine and the screen
 * stays quiet. One presence, one line of words, one way out.
 *
 * The capsule hugs its words (280 to 360 wide) and only changes width when the phase does,
 * over --duration-base. While Ghost speaks it is a caption at the full 360: no title (the
 * presence's bars already say "speaking"), just the newest words of the reply on up to two
 * lines, the same height as every other phase. If the daemon drops mid-session the HUD
 * stays, as Reconnecting, because Ghost did not go to sleep.
 *
 * Screen readers: the capsule is a labeled group, not a live region. A separate status
 * line announces the few changes worth hearing (Listening, Waiting for your okay, Microphone
 * off, Reconnecting); the streaming caption and thinking/speaking flips stay silent.
 */
import { useGhost } from "@ghost/ui";
import { Moon } from "lucide-react";
import { useLayoutEffect, useRef, useState } from "react";

import { IconButton, VoicePresence, cx, quoted, useLive } from "../../ui";
import type { TrayState } from "./model";
import { tokenMs } from "./motion";

/** Two lines of the caption's text column at 13px hold about 70 characters. */
const CAPTION_CHARS = 64;

/** The tail of what Ghost is saying, from a word boundary, so the newest words always show. */
export function captionTail(text: string, max = CAPTION_CHARS): string {
  const t = text.trim();
  if (t.length <= max) return t;
  const cut = t.slice(t.length - max);
  const space = cut.indexOf(" ");
  return `…${space >= 0 && space < 24 ? cut.slice(space + 1) : cut}`;
}

/**
 * Ease the capsule's width from where it was to where it is, when `key` (the phase) changes.
 * Width is content-sized, which CSS can't transition, so measure and animate it once.
 */
function useWidthEase(root: React.RefObject<HTMLElement | null>, key: string) {
  const last = useRef<number | null>(null);
  useLayoutEffect(() => {
    const el = root.current;
    if (!el) {
      last.current = null;
      return;
    }
    const to = el.getBoundingClientRect().width;
    const from = last.current;
    last.current = to;
    const ms = tokenMs(el, "--duration-base");
    if (from == null || Math.abs(from - to) < 1 || !ms || !el.animate) return;
    el.animate([{ width: `${from}px` }, { width: `${to}px` }], {
      duration: ms,
      easing: getComputedStyle(el).getPropertyValue("--ease-out").trim() || "ease-out",
    });
  }, [root, key]);
}

export function TrayHud({
  state,
  onSleepStart,
}: {
  state: TrayState;
  /**
   * Go to sleep was pressed. The HUD is about to disappear with the button in it, so the
   * stage moves keyboard focus to the top-bar indicator once it does.
   */
  onSleepStart: () => void;
}) {
  const { client, turns, pending } = useGhost();
  const { online } = useLive();
  const [busy, setBusy] = useState(false);
  const root = useRef<HTMLDivElement>(null);

  const lastSaid = [...turns].reverse().find((t) => t.role === "assistant")?.text ?? null;
  let title: string | null = state.title;
  let sub = state.sub;
  let tone: "warn" | null = null;
  let caption = false;
  let kind: string = state.presence;
  if (state.stale) {
    title = "Reconnecting";
    sub = "Ghost can’t hear you right now";
    tone = "warn";
    kind = "stale";
  } else if (pending) {
    title = "Waiting for your okay";
    sub = state.muted ? "Mic off · Confirm in the menu" : `Say ${quoted("yes")} or ${quoted("no")}`;
    tone = "warn";
    kind = state.muted ? "pending-muted" : "pending";
  } else if (state.muted) {
    sub = "Ghost can’t hear you";
    kind = "muted";
  } else if (state.presence === "speaking" && lastSaid) {
    title = null;
    sub = captionTail(lastSaid);
    caption = true;
  } else {
    sub = state.where;
  }

  // What a screen reader hears: only the changes that matter to someone not looking.
  const announce = !state.inSession
    ? ""
    : state.stale
      ? "Reconnecting"
      : pending
        ? "Waiting for your okay"
        : state.muted
          ? "Microphone off"
          : "Listening";

  useWidthEase(root, state.inSession ? `${kind}:${state.where}` : "off");

  const sleep = async () => {
    onSleepStart();
    setBusy(true);
    try {
      await client.sleep();
    } catch {
      /* the popover shows errors; the HUD stays a glance */
    } finally {
      setBusy(false);
    }
  };

  return (
    <>
      <p className="sr-only" role="status">
        {announce}
      </p>
      {state.inSession && (
        <div ref={root} className={cx("tray-hud", caption && "is-caption")} role="group" aria-label="Ghost is awake">
          <VoicePresence phase={state.presence} size={48} label="" />
          <div className="tray-hud__text">
            {title && <p className={cx("tray-hud__title", tone === "warn" && "t-warn")}>{title}</p>}
            {caption && <span className="sr-only">Ghost is saying: </span>}
            <p
              className="tray-hud__sub"
              aria-live="off"
              data-user-content={caption ? "" : undefined}
              title={caption ? (lastSaid ?? undefined) : undefined}
            >
              {sub}
            </p>
          </div>
          {/* A real button at a glance, not a glyph beside the presence: a hairline circle on a
              quiet surface, as the way out of a system pill. */}
          <IconButton
            label="Go to sleep"
            icon={<Moon />}
            variant="secondary"
            className="tray-hud__sleep"
            disabled={!online}
            loading={busy}
            onClick={sleep}
          />
        </div>
      )}
    </>
  );
}
