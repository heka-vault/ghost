import { Check, Minus } from "lucide-react";
import { type ReactNode, useEffect, useId, useRef } from "react";

import { cx } from "./cx";
import { useFieldControl } from "./Field";

/* ------------------------------------------------------------------------ Checkbox */
export interface CheckboxProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** Shows a dash: some, not all, of a group is on. Clicking sets it to checked. */
  indeterminate?: boolean;
  label?: ReactNode;
  /** One plain sentence under the label. */
  description?: ReactNode;
  disabled?: boolean;
  name?: string;
  value?: string;
  id?: string;
  className?: string;
  "aria-label"?: string;
  "aria-describedby"?: string;
}

/**
 * A checkbox on a native input, so forms, Space and screen readers behave. Use it for
 * choices that take effect on Save; a Switch is for settings that apply at once.
 */
export function Checkbox({ checked, onChange, indeterminate, label, description, disabled, name, value, className, ...rest }: CheckboxProps) {
  const a = useFieldControl(rest);
  const ref = useRef<HTMLInputElement>(null);
  const descId = useId();
  useEffect(() => {
    if (ref.current) ref.current.indeterminate = !!indeterminate;
  }, [indeterminate]);
  const box = (
    <span className={cx("g-check__box", (checked || indeterminate) && "is-on")}>
      <input
        ref={ref}
        type="checkbox"
        className="g-check__input"
        checked={checked}
        disabled={disabled}
        name={name}
        value={value}
        id={a.id}
        aria-label={rest["aria-label"]}
        aria-describedby={cx(a["aria-describedby"], description ? descId : undefined) || undefined}
        aria-checked={indeterminate ? "mixed" : undefined}
        onChange={(e) => onChange(e.target.checked)}
      />
      {indeterminate ? <Minus className="g-check__mark" strokeWidth={2.5} aria-hidden /> : checked ? <Check className="g-check__mark" strokeWidth={2.5} aria-hidden /> : null}
    </span>
  );
  if (!label) return <span className={cx("g-check", disabled && "is-disabled", className)}>{box}</span>;
  return (
    <label className={cx("g-check", disabled && "is-disabled", className)}>
      {box}
      <span className="g-check__text">
        <span className="g-check__label">{label}</span>
        {description && <span id={descId} className="g-check__description">{description}</span>}
      </span>
    </label>
  );
}

/* ---------------------------------------------------------------------- RadioGroup */
export interface RadioOption<T extends string> {
  value: T;
  label: ReactNode;
  description?: ReactNode;
  /** Card variant only: a leading icon or glyph. The radio then moves to the trailing edge, so each card leads with one marker. */
  icon?: ReactNode;
  /** Card variant only: right-aligned small text or a Badge. */
  aside?: ReactNode;
  disabled?: boolean;
}

export interface RadioGroupProps<T extends string> {
  options: RadioOption<T>[];
  value: T | null;
  onChange: (value: T) => void;
  /** Visible group label; or pass aria-label. */
  label?: ReactNode;
  "aria-label"?: string;
  /** `list`: compact radios with labels. `card`: bordered options with title and description. */
  variant?: "list" | "card";
  /** Card variant: lay cards out in N columns (default 1, stacked). */
  columns?: 1 | 2 | 3 | 4;
  disabled?: boolean;
  name?: string;
  className?: string;
}

/**
 * One choice from 2 to 6 options that need more words than a SegmentedControl allows.
 * Native radios: Tab enters the group at the selected option, arrow keys move and select.
 */
export function RadioGroup<T extends string>({ options, value, onChange, label, variant = "list", columns = 1, disabled, name, className, ...rest }: RadioGroupProps<T>) {
  const auto = useId();
  const group = name ?? `rg${auto}`;
  const labelId = `${group}-label`;
  return (
    <div
      role="radiogroup"
      aria-labelledby={label ? labelId : undefined}
      aria-label={label ? undefined : rest["aria-label"]}
      aria-disabled={disabled || undefined}
      className={cx("g-radios", `g-radios--${variant}`, variant === "card" && columns > 1 && `g-radios--cols-${columns}`, className)}
    >
      {label && <div id={labelId} className="g-radios__label">{label}</div>}
      {options.map((o) => {
        const on = o.value === value;
        const off = disabled || o.disabled;
        const descId = o.description ? `${group}-${o.value}-desc` : undefined;
        return (
          <label key={o.value} className={cx(variant === "card" ? "g-radio-card" : "g-radio", variant === "card" && o.icon != null && "has-icon", on && "is-on", off && "is-disabled")}>
            <span className="g-radio__dot">
              <input
                type="radio"
                className="g-radio__input"
                name={group}
                value={o.value}
                checked={on}
                disabled={off}
                aria-describedby={descId}
                onChange={() => onChange(o.value)}
              />
            </span>
            {variant === "card" && o.icon && <span className="g-radio-card__icon" aria-hidden>{o.icon}</span>}
            <span className="g-radio__text">
              <span className="g-radio__label">{o.label}</span>
              {o.description && <span id={descId} className="g-radio__description">{o.description}</span>}
            </span>
            {variant === "card" && o.aside && <span className="g-radio-card__aside">{o.aside}</span>}
          </label>
        );
      })}
    </div>
  );
}
