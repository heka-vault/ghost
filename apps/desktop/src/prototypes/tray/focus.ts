/**
 * Owner: tray. Keyboard focus that survives Ghost's actions. Wake, Confirm and Sleep
 * all unmount the button that was pressed; without help focus falls to <body> and a keyboard
 * user loses their place right after the most consequential thing they did.
 *
 * Focus moves only for someone using the keyboard (their last input was a key, not the
 * pointer), so mouse users never see a ring appear on its own.
 */
import { type RefObject, useCallback, useEffect, useRef } from "react";

let keyboard = false;
if (typeof window !== "undefined") {
  window.addEventListener(
    "keydown",
    (e) => {
      if (!e.metaKey && !e.ctrlKey && !e.altKey) keyboard = true;
    },
    true,
  );
  window.addEventListener("pointerdown", () => (keyboard = false), true);
}

/** The operator's last input was the keyboard. */
export const usingKeyboard = (): boolean => keyboard;

/** Focus was lost: nothing, or <body>, or an element that left the document. */
function lost(): boolean {
  const a = document.activeElement;
  return !a || a === document.body || !a.isConnected;
}

const ACTIONABLE =
  'button:not([disabled]):not([aria-hidden="true"]), a[href], [role="menuitem"]:not([aria-disabled="true"]), [role="menuitemcheckbox"]:not([aria-disabled="true"])';

/** The controls Up and Down move between, in document order, skipping hidden ones. */
export function actionables(root: HTMLElement | null): HTMLElement[] {
  if (!root) return [];
  return [...root.querySelectorAll<HTMLElement>(ACTIONABLE)].filter(
    (el) => !(el as HTMLButtonElement).disabled && (el.offsetParent !== null || el === document.activeElement),
  );
}

/** Up/Down (and Home/End) move between a menu's controls, wrapping, as GNOME menus do. */
export function roveKey(e: React.KeyboardEvent<HTMLElement>, root: HTMLElement | null): void {
  if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
  const items = actionables(root);
  if (!items.length) return;
  // A switch is a button; Up/Down never mean anything else inside this menu.
  e.preventDefault();
  const i = items.indexOf(document.activeElement as HTMLElement);
  const next =
    e.key === "Home"
      ? 0
      : e.key === "End"
        ? items.length - 1
        : e.key === "ArrowDown"
          ? (i + 1) % items.length
          : (i <= 0 ? items.length : i) - 1;
  items[next].focus();
}

/**
 * Within `root`, put focus back after the focused control unmounts. `request(target)` names
 * where it should go after the next action (call it as the action starts); if focus is later
 * lost while it was inside `root`, it goes to that target, or to `fallback()`. A request is
 * forgotten after a few seconds, so focus never jumps long after the fact.
 */
export function useFocusKeeper(root: RefObject<HTMLElement | null>, fallback: () => HTMLElement | null) {
  const wanted = useRef<{ target: () => HTMLElement | null; until: number } | null>(null);
  const inside = useRef(false);
  const fallbackRef = useRef(fallback);
  fallbackRef.current = fallback;

  const settle = useCallback(() => {
    if (!keyboard || !inside.current || !lost()) return;
    const w = wanted.current && wanted.current.until > Date.now() ? wanted.current : null;
    const el = w?.target() ?? fallbackRef.current();
    if (el) {
      el.focus();
      if (document.activeElement === el) wanted.current = null;
    }
  }, []);

  // Track whether focus is in our surface: focusin says yes; a focusout to somewhere else
  // says no. A focused button that unmounts or turns disabled fires nothing, so it stays yes.
  useEffect(() => {
    const el = root.current;
    if (!el) return;
    const onIn = () => (inside.current = true);
    const onOut = (e: FocusEvent) => {
      const to = e.relatedTarget as Node | null;
      if (to && !el.contains(to)) inside.current = false;
    };
    el.addEventListener("focusin", onIn);
    el.addEventListener("focusout", onOut);
    // Unmounts and disabled buttons happen between our renders too (the change's clock).
    const t = window.setInterval(settle, 200);
    return () => {
      el.removeEventListener("focusin", onIn);
      el.removeEventListener("focusout", onOut);
      window.clearInterval(t);
    };
  });

  useEffect(settle);

  const request = useCallback(
    (target: () => HTMLElement | null) => {
      if (!keyboard) return;
      wanted.current = { target, until: Date.now() + 4000 };
      requestAnimationFrame(settle);
    },
    [settle],
  );
  return { request, settle };
}
