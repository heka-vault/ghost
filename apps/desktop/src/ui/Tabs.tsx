import { type KeyboardEvent, type ReactNode, useId, useRef } from "react";

import { cx } from "./cx";

export interface TabItem<T extends string> {
  value: T;
  label: ReactNode;
  icon?: ReactNode;
  /** A small count after the label (e.g. pending items). */
  count?: number;
  disabled?: boolean;
}

export interface TabsProps<T extends string> {
  items: TabItem<T>[];
  value: T;
  onChange: (value: T) => void;
  "aria-label": string;
  size?: "sm" | "md";
  /** The selected tab's content, rendered in a labeled tabpanel. Omit to render the tab list only. */
  children?: ReactNode;
  className?: string;
}

/**
 * Switches between views of the same object (a line's Persona / Memory / Transcript).
 * Roving tabindex: Tab enters at the selected tab, arrow keys and Home/End move and select.
 * For 2 to 4 settings values use SegmentedControl; for navigation between pages use routes.
 */
export function Tabs<T extends string>({ items, value, onChange, size = "md", children, className, ...rest }: TabsProps<T>) {
  const base = useId();
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const enabled = items.map((t, i) => (t.disabled ? -1 : i)).filter((i) => i >= 0);
  const tabId = (v: string) => `${base}-tab-${v}`;
  const panelId = `${base}-panel`;

  const onKey = (e: KeyboardEvent, i: number) => {
    let next = -1;
    const k = enabled.indexOf(i);
    if (e.key === "ArrowRight") next = enabled[(k + 1) % enabled.length];
    if (e.key === "ArrowLeft") next = enabled[(k - 1 + enabled.length) % enabled.length];
    if (e.key === "Home") next = enabled[0];
    if (e.key === "End") next = enabled[enabled.length - 1];
    if (next >= 0) {
      e.preventDefault();
      onChange(items[next].value);
      refs.current[next]?.focus();
    }
  };

  return (
    <div className={cx("g-tabs", `g-tabs--${size}`, className)}>
      <div role="tablist" aria-label={rest["aria-label"]} className="g-tabs__list">
        {items.map((t, i) => {
          const on = t.value === value;
          return (
            <button
              key={t.value}
              ref={(el) => {
                refs.current[i] = el;
              }}
              id={tabId(t.value)}
              type="button"
              role="tab"
              aria-selected={on}
              aria-controls={children != null ? panelId : undefined}
              tabIndex={on ? 0 : -1}
              disabled={t.disabled}
              className={cx("g-tabs__tab", on && "is-on")}
              onClick={() => onChange(t.value)}
              onKeyDown={(e) => onKey(e, i)}
            >
              {t.icon}
              <span>{t.label}</span>
              {t.count != null && <span className="g-tabs__count t-num">{t.count}</span>}
            </button>
          );
        })}
      </div>
      {children != null && (
        <div role="tabpanel" id={panelId} aria-labelledby={tabId(value)} tabIndex={0} className="g-tabs__panel">
          {children}
        </div>
      )}
    </div>
  );
}
