import {
  cloneElement,
  type KeyboardEvent,
  type ReactElement,
  type ReactNode,
  useCallback,
  useEffect,
  useId,
  useLayoutEffect,
  useRef,
  useState,
} from "react";
import { createPortal } from "react-dom";

import { cx } from "./cx";
import { formatShortcut } from "./format";

export interface MenuAction {
  label: string;
  onSelect: () => void;
  icon?: ReactNode;
  /** A shortcut in formatShortcut() notation, e.g. "mod+backspace". Shown, not bound. */
  shortcut?: string;
  /** Destructive: red text. Still open a ConfirmDialog before deleting anything. */
  danger?: boolean;
  disabled?: boolean;
  /** Why a disabled item can't be chosen now, under its label (“Ghost is on this line”). */
  reason?: string;
}
export type MenuItem = MenuAction | "separator" | { heading: string };

export interface MenuProps {
  /** The trigger: a Button or IconButton. Menu adds aria-haspopup, aria-expanded and the handlers. */
  trigger: ReactElement;
  items: MenuItem[];
  /** Horizontal alignment to the trigger. Row overflow menus use "end". */
  align?: "start" | "end";
  /** Accessible name of the menu; defaults to the trigger's label. */
  "aria-label"?: string;
  className?: string;
}

const isAction = (i: MenuItem): i is MenuAction => typeof i === "object" && "label" in i;

/**
 * A dropdown of actions for one object (row overflow: Rename, Reset, Delete). Opens on
 * click, Enter, Space or ArrowDown; arrow keys, Home/End and typeahead move; Enter selects;
 * Escape or Tab closes and focus returns to the trigger. Not for navigation or choosing a
 * value (use Select or RadioGroup).
 */
export function Menu({ trigger, items, align = "start", className, ...rest }: MenuProps) {
  const id = useId();
  const anchor = useRef<HTMLSpanElement>(null);
  const menu = useRef<HTMLDivElement>(null);
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(-1);
  const [pos, setPos] = useState<{ x: number; y: number } | null>(null);
  const typed = useRef({ text: "", at: 0 });
  const enabled = items.map((it, i) => (isAction(it) && !it.disabled ? i : -1)).filter((i) => i >= 0);

  const triggerEl = () => anchor.current?.querySelector<HTMLElement>("button, [tabindex]") ?? null;

  const close = useCallback((refocus = true) => {
    setOpen(false);
    setPos(null);
    setActive(-1);
    if (refocus) triggerEl()?.focus({ preventScroll: true });
  }, []);

  // When the menu last opened. Scroll events in the first frame after opening come from
  // the opening itself (focus, layout) and must not close it.
  const openedAt = useRef(0);

  const openAt = (which: "first" | "last") => {
    openedAt.current = performance.now();
    setOpen(true);
    setActive(which === "first" ? (enabled[0] ?? -1) : (enabled[enabled.length - 1] ?? -1));
  };

  useLayoutEffect(() => {
    if (!open || !anchor.current || !menu.current) return;
    const t = triggerEl() ?? anchor.current;
    const a = t.getBoundingClientRect();
    const m = menu.current.getBoundingClientRect();
    const gap = 4;
    let x = align === "end" ? a.right - m.width : a.left;
    let y = a.bottom + gap;
    if (y + m.height > window.innerHeight - 8) y = Math.max(8, a.top - m.height - gap);
    x = Math.max(8, Math.min(x, window.innerWidth - m.width - 8));
    setPos({ x, y });
  }, [open, align]);

  useEffect(() => {
    if (open && active >= 0) refs.current[active]?.focus({ preventScroll: true });
  }, [open, active, pos]);

  useEffect(() => {
    if (!open) return;
    const onDown = (e: PointerEvent) => {
      const t = e.target as Node;
      if (menu.current?.contains(t) || anchor.current?.contains(t)) return;
      close(false);
    };
    const onScroll = (e: Event) => {
      if (menu.current?.contains(e.target as Node)) return;
      if (performance.now() - openedAt.current < 50) return;
      close(false);
    };
    const onResize = () => close(false);
    document.addEventListener("pointerdown", onDown, true);
    window.addEventListener("scroll", onScroll, true);
    window.addEventListener("resize", onResize);
    return () => {
      document.removeEventListener("pointerdown", onDown, true);
      window.removeEventListener("scroll", onScroll, true);
      window.removeEventListener("resize", onResize);
    };
  }, [open, close]);

  const move = (dir: 1 | -1) => {
    if (!enabled.length) return;
    const k = enabled.indexOf(active);
    setActive(enabled[(k + dir + enabled.length) % enabled.length]);
  };

  const select = (i: number) => {
    const it = items[i];
    if (!isAction(it) || it.disabled) return;
    close(true);
    it.onSelect();
  };

  const onMenuKey = (e: KeyboardEvent) => {
    const k = e.key;
    if (k === "ArrowDown") move(1);
    else if (k === "ArrowUp") move(-1);
    else if (k === "Home") setActive(enabled[0] ?? -1);
    else if (k === "End") setActive(enabled[enabled.length - 1] ?? -1);
    else if (k === "Escape") close(true);
    else if (k === "Tab") return close(false);
    else if (k === "Enter" || k === " ") select(active);
    else if (k.length === 1 && /\S/.test(k)) {
      const now = Date.now();
      typed.current = { text: (now - typed.current.at > 600 ? "" : typed.current.text) + k.toLowerCase(), at: now };
      const q = typed.current.text;
      const start = enabled.indexOf(active);
      const order = [...enabled.slice(start + 1), ...enabled.slice(0, start + 1)];
      const hit = (q.length > 1 ? enabled : order).find((i) => (items[i] as MenuAction).label.toLowerCase().startsWith(q));
      if (hit != null) setActive(hit);
    } else return;
    e.preventDefault();
  };

  const t = trigger.props as Record<string, unknown>;
  const label = rest["aria-label"] ?? (t.label as string | undefined) ?? (t["aria-label"] as string | undefined);
  const triggerNode = cloneElement(trigger as ReactElement<Record<string, unknown>>, {
    "aria-haspopup": "menu",
    "aria-expanded": open,
    "aria-controls": open ? id : undefined,
    pressed: undefined,
    className: cx(t.className as string | undefined, open && "is-active"),
    // Space activates a button on keyup; the keydown above already acted.
    onKeyUp: (e: KeyboardEvent) => {
      (t.onKeyUp as ((e: KeyboardEvent) => void) | undefined)?.(e);
      if (e.key === " ") e.preventDefault();
    },
    onClick: (e: unknown) => {
      (t.onClick as ((e: unknown) => void) | undefined)?.(e);
      if (open) close(false);
      else openAt("first");
    },
    onKeyDown: (e: KeyboardEvent) => {
      (t.onKeyDown as ((e: KeyboardEvent) => void) | undefined)?.(e);
      // Enter and Space are handled here, not left to the browser: its synthesized click
      // lands after focus has moved into the menu, which closed it and dropped focus.
      if (e.key === "Enter" || e.key === " ") {
        e.preventDefault();
        if (e.repeat) return;
        if (open) close(true);
        else openAt("first");
      } else if (e.key === "ArrowDown") {
        e.preventDefault();
        openAt("first");
      } else if (e.key === "ArrowUp") {
        e.preventDefault();
        openAt("last");
      }
    },
  });

  return (
    <span ref={anchor} className="g-menu-anchor">
      {triggerNode}
      {open &&
        createPortal(
          <div
            ref={menu}
            id={id}
            role="menu"
            aria-label={label}
            tabIndex={-1}
            className={cx("g-menu", pos && "is-placed", className)}
            style={pos ? { left: pos.x, top: pos.y } : { left: -9999, top: -9999 }}
            onKeyDown={onMenuKey}
          >
            {items.map((it, i) => {
              if (it === "separator") return <div key={i} role="separator" className="g-menu__separator" />;
              if (!isAction(it)) return <div key={i} role="presentation" className="g-menu__heading">{it.heading}</div>;
              return (
                <button
                  key={i}
                  ref={(el) => {
                    refs.current[i] = el;
                  }}
                  type="button"
                  role="menuitem"
                  tabIndex={-1}
                  aria-disabled={it.disabled || undefined}
                  className={cx("g-menu__item", it.danger && "is-danger", it.disabled && "is-disabled", it.disabled && it.reason && "has-reason", i === active && "is-active")}
                  aria-describedby={it.disabled && it.reason ? `${id}-reason-${i}` : undefined}
                  onPointerMove={() => !it.disabled && active !== i && setActive(i)}
                  onClick={() => select(i)}
                >
                  <span className="g-menu__icon" aria-hidden>{it.icon}</span>
                  <span className="g-menu__text">
                    <span className="g-menu__label">{it.label}</span>
                    {it.disabled && it.reason && <span id={`${id}-reason-${i}`} className="g-menu__reason">{it.reason}</span>}
                  </span>
                  {/* Plain muted text, not key caps: caps on every row are noise (Linear, Coda). */}
                  {it.shortcut && <span className="g-menu__shortcut t-num">{formatShortcut(it.shortcut)}</span>}
                </button>
              );
            })}
          </div>,
          document.body,
        )}
    </span>
  );
}
