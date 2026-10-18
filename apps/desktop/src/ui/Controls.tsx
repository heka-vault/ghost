import { type KeyboardEvent, type ReactNode, useContext, useRef } from "react";

import { ButtonSizeContext } from "./Button";

import { cx } from "./cx";
import { useFieldControl } from "./Field";

export interface SwitchProps {
  checked: boolean;
  onChange: (checked: boolean) => void;
  /** Visible label to the right. Omit when a Field or ListRow labels it (pass aria-label then). */
  label?: ReactNode;
  disabled?: boolean;
  size?: "sm" | "md";
  id?: string;
  className?: string;
  "aria-label"?: string;
  "aria-describedby"?: string;
}

export function Switch({ checked, onChange, label, disabled, size = "md", className, ...rest }: SwitchProps) {
  const a = useFieldControl(rest);
  const button = (
    <button
      type="button"
      role="switch"
      aria-checked={checked}
      aria-label={rest["aria-label"]}
      id={a.id}
      aria-describedby={a["aria-describedby"]}
      disabled={disabled}
      className={cx("g-switch", `g-switch--${size}`, className)}
      onClick={() => onChange(!checked)}
    >
      <span className="g-switch__thumb" />
    </button>
  );
  if (!label) return button;
  return (
    <label className={cx("g-switch-label", disabled && "is-disabled")}>
      {button}
      <span>{label}</span>
    </label>
  );
}

export interface SegmentedOption<T extends string> {
  value: T;
  label: ReactNode;
  icon?: ReactNode;
  disabled?: boolean;
}

export interface SegmentedControlProps<T extends string> {
  options: SegmentedOption<T>[];
  value: T;
  onChange: (value: T) => void;
  /** The accessible name. Inside a Field the Field's label names it instead. */
  "aria-label"?: string;
  size?: "sm" | "md";
  fullWidth?: boolean;
  disabled?: boolean;
  /** Set by Field (id, aria-describedby, aria-labelledby); pass them through when used inside one. */
  id?: string;
  "aria-describedby"?: string;
  "aria-labelledby"?: string;
  className?: string;
}

/** A radiogroup of 2-5 mutually exclusive choices. Arrow keys move and select. Works as a Field's control. */
export function SegmentedControl<T extends string>({ options, value, onChange, size: sizeProp, fullWidth, disabled, className, ...rest }: SegmentedControlProps<T>) {
  // In the compact page bar (ButtonSizeContext "sm") it shrinks with the buttons beside it.
  const ctx = useContext(ButtonSizeContext);
  const size = sizeProp ?? (ctx === "sm" ? "sm" : "md");
  const a = useFieldControl(rest);
  const refs = useRef<(HTMLButtonElement | null)[]>([]);
  const enabled = options.map((o, i) => (o.disabled || disabled ? -1 : i)).filter((i) => i >= 0);
  const onKey = (e: KeyboardEvent, i: number) => {
    const dir = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    let next = -1;
    if (dir) next = enabled[(enabled.indexOf(i) + dir + enabled.length) % enabled.length];
    if (e.key === "Home") next = enabled[0];
    if (e.key === "End") next = enabled[enabled.length - 1];
    if (next >= 0) {
      e.preventDefault();
      onChange(options[next].value);
      refs.current[next]?.focus();
    }
  };
  return (
    <div role="radiogroup" id={a.id} aria-label={rest["aria-label"]} aria-labelledby={rest["aria-labelledby"]} aria-describedby={a["aria-describedby"]} aria-invalid={a["aria-invalid"]} className={cx("g-seg", `g-seg--${size}`, fullWidth && "g-seg--full", className)}>
      {options.map((o, i) => {
        const on = o.value === value;
        return (
          <button
            key={o.value}
            ref={(el) => {
              refs.current[i] = el;
            }}
            type="button"
            role="radio"
            aria-checked={on}
            tabIndex={on ? 0 : -1}
            disabled={o.disabled || disabled}
            className={cx("g-seg__item", on && "is-on")}
            onClick={() => onChange(o.value)}
            onKeyDown={(e) => onKey(e, i)}
          >
            {o.icon}
            {o.label != null && <span>{o.label}</span>}
          </button>
        );
      })}
    </div>
  );
}

export interface SliderProps {
  value: number;
  onChange: (value: number) => void;
  min?: number;
  max?: number;
  step?: number;
  disabled?: boolean;
  /** Formats the readout to the right; omit to hide it. */
  format?: (v: number) => string;
  /** A fixed width for the readout (a CSS length), so the track never moves as the words change. */
  readoutWidth?: string | number;
  id?: string;
  "aria-label"?: string;
  "aria-describedby"?: string;
}

export function Slider({ value, onChange, min = 0, max = 100, step = 1, disabled, format, readoutWidth, ...rest }: SliderProps) {
  const a = useFieldControl(rest);
  const pct = ((value - min) / (max - min)) * 100;
  return (
    <div className={cx("g-slider", disabled && "is-disabled")}>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        aria-label={rest["aria-label"]}
        aria-valuetext={format ? format(value) : undefined}
        {...a}
        style={{ ["--pct" as string]: `${pct}%` }}
        onChange={(e) => onChange(Number(e.target.value))}
      />
      {format && <output className="g-slider__value t-num" style={readoutWidth != null ? { minWidth: readoutWidth } : undefined}>{format(value)}</output>}
    </div>
  );
}
