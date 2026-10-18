import { type ReactElement, type ReactNode, type RefObject, cloneElement, useEffect, useId, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";

import { cx } from "./cx";
import { formatShortcut } from "./format";

export interface TooltipProps {
  content: ReactNode;
  children: ReactNode;
  side?: "top" | "bottom" | "left" | "right";
  /** Delay before showing on hover, ms. Keyboard focus shows it at once. */
  delay?: number;
  /** Optional shortcut shown after the text, in formatShortcut() notation ("mod+z" reads "Ctrl Z"). */
  shortcut?: string;
  /**
   * Attach to the child element itself instead of wrapping it in a span: for a child whose
   * flex sizing (a truncating label) the wrapper would break. The child must take a ref and
   * the pointer and focus handlers.
   */
  asChild?: boolean;
}

/**
 * A short label for an icon or a truncated value. Never put actions or essential
 * information in a tooltip: it does not exist on touch and is easy to miss.
 * Renders into a portal so it is never clipped by a scroller.
 */
export function Tooltip({ content, children, side = "top", delay = 450, shortcut, asChild }: TooltipProps) {
  const id = useId();
  const anchor = useRef<HTMLElement>(null);
  const bubble = useRef<HTMLDivElement>(null);
  const timer = useRef<ReturnType<typeof setTimeout> | null>(null);
  const [open, setOpen] = useState(false);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);

  const show = (now = false) => {
    if (timer.current) clearTimeout(timer.current);
    timer.current = setTimeout(() => setOpen(true), now ? 0 : delay);
  };
  const hide = () => {
    if (timer.current) clearTimeout(timer.current);
    setOpen(false);
    setPos(null);
  };
  useEffect(() => () => { if (timer.current) clearTimeout(timer.current); }, []);
  // The description belongs on the focusable trigger, not on the wrapper, or screen readers
  // never announce it. The trigger is the anchor's first element child (or the anchor itself with asChild).
  const triggerEl = () => (asChild ? anchor.current : (anchor.current?.firstElementChild as HTMLElement | null));
  useEffect(() => {
    const target = triggerEl();
    if (!target) return;
    if (open) {
      const prev = target.getAttribute("aria-describedby");
      if (!prev?.split(" ").includes(id)) target.setAttribute("aria-describedby", prev ? `${prev} ${id}` : id);
    } else {
      const left = (target.getAttribute("aria-describedby") ?? "").split(" ").filter((x) => x && x !== id);
      if (left.length) target.setAttribute("aria-describedby", left.join(" "));
      else target.removeAttribute("aria-describedby");
    }
  }, [open, id]);
  useEffect(() => {
    if (!open) return;
    const onKey = (e: KeyboardEvent) => e.key === "Escape" && hide();
    window.addEventListener("keydown", onKey);
    // Capture phase: hears scrolls of the page panel (.shell__main) and any inner scroller.
    window.addEventListener("scroll", hide, true);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("scroll", hide, true);
    };
  }, [open]);

  useLayoutEffect(() => {
    if (!open || !anchor.current || !bubble.current) return;
    const target = triggerEl() ?? anchor.current;
    const a = target.getBoundingClientRect();
    const b = bubble.current.getBoundingClientRect();
    const gap = 6;
    let x = a.left + a.width / 2 - b.width / 2;
    let y = a.top - b.height - gap;
    if (side === "bottom") y = a.bottom + gap;
    if (side === "left") { x = a.left - b.width - gap; y = a.top + a.height / 2 - b.height / 2; }
    if (side === "right") { x = a.right + gap; y = a.top + a.height / 2 - b.height / 2; }
    x = Math.max(8, Math.min(x, window.innerWidth - b.width - 8));
    y = Math.max(8, Math.min(y, window.innerHeight - b.height - 8));
    setPos({ x, y });
  }, [open, side]);

  if (content == null || content === "") return <>{children}</>;
  const handlers = {
    onPointerEnter: () => show(),
    onPointerLeave: hide,
    onPointerDown: hide,
    onFocus: (e: { target: EventTarget }) => (e.target as HTMLElement).matches(":focus-visible") && show(true),
    onBlur: hide,
  };
  const bubble_ =
    open &&
    createPortal(
      <div ref={bubble} id={id} role="tooltip" className={cx("g-tooltip", pos && "is-placed")} style={pos ? { left: pos.x, top: pos.y } : { left: -9999, top: -9999 }}>
        {content}
        {shortcut && <span className="g-tooltip__shortcut">{formatShortcut(shortcut)}</span>}
      </div>,
      document.body,
    );
  if (asChild) {
    const child = children as ReactElement<Record<string, unknown>>;
    return (
      <>
        {cloneElement(child, { ...handlers, ref: anchor })}
        {bubble_}
      </>
    );
  }
  return (
    <span ref={anchor as RefObject<HTMLSpanElement>} className="g-tooltip-anchor" {...handlers}>
      {children}
      {open &&
        createPortal(
          <div
            ref={bubble}
            id={id}
            role="tooltip"
            className={cx("g-tooltip", pos && "is-placed")}
            style={pos ? { left: pos.x, top: pos.y } : { left: -9999, top: -9999 }}
          >
            {content}
            {shortcut && <span className="g-tooltip__shortcut">{formatShortcut(shortcut)}</span>}
          </div>,
          document.body,
        )}
    </span>
  );
}
