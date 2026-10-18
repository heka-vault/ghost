import { ChevronDown } from "lucide-react";
import {
  type ComponentPropsWithRef,
  type CSSProperties,
  createContext,
  type ReactNode,
  useContext,
  useEffect,
  useId,
  useRef,
} from "react";

import { cx } from "./cx";

interface FieldCtx {
  id: string;
  describedBy?: string;
  invalid: boolean;
  disabled?: boolean;
}
const FieldContext = createContext<FieldCtx | null>(null);

/** Controls call this to pick up the id, aria-describedby and aria-invalid of their Field. */
export function useFieldControl(props: { id?: string; "aria-describedby"?: string; "aria-invalid"?: unknown }) {
  const f = useContext(FieldContext);
  return {
    id: props.id ?? f?.id,
    "aria-describedby": cx(props["aria-describedby"], f?.describedBy) || undefined,
    "aria-invalid": (props["aria-invalid"] as boolean | undefined) ?? (f?.invalid || undefined),
  };
}

export interface FieldProps {
  label: ReactNode;
  /** One plain sentence under the control. */
  help?: ReactNode;
  /** Replaces help while present; turns the control red. */
  error?: ReactNode;
  /** Secondary technical detail in mono, e.g. the environment variable name. */
  detail?: ReactNode;
  /** Right side of the label row (e.g. a value readout or a "Set" badge). */
  aside?: ReactNode;
  optional?: boolean;
  /** `stack`: label above control. `inline`: label and help left, control right (settings rows). */
  layout?: "stack" | "inline";
  /**
   * Inline only: width of the control column, so controls line up down a Card. Default 320
   * (inputs, selects); use "auto" for a Switch or a small button.
   */
  controlWidth?: number | "auto";
  id?: string;
  className?: string;
  children: ReactNode;
}

export function Field({ label, help, error, detail, aside, optional, layout = "stack", controlWidth, id, className, children }: FieldProps) {
  const auto = useId();
  const controlId = id ?? `f${auto}`;
  const helpId = help || error ? `${controlId}-help` : undefined;
  useEmptyFieldCheck(controlId);
  const text = (
    <div className="g-field__text">
      <div className="g-field__label-row">
        <label className="g-field__label" htmlFor={controlId}>
          {label}
          {optional && <span className="g-field__optional"> Optional</span>}
        </label>
        {detail && <span className="g-field__detail">{detail}</span>}
        {aside && <span className="g-field__aside">{aside}</span>}
      </div>
      {layout === "inline" && (error || help) && (
        <p id={helpId} className={cx("g-field__help", error && "is-error")}>{error ?? help}</p>
      )}
    </div>
  );
  const control = <div className="g-field__control">{children}</div>;
  return (
    <FieldContext.Provider value={{ id: controlId, describedBy: helpId, invalid: !!error }}>
      {layout === "inline" ? (
        <div
          className={cx("g-field", "g-field--inline", controlWidth === "auto" && "is-auto", error && "is-invalid", className)}
          style={controlWidth != null ? ({ "--field-control-width": controlWidth === "auto" ? "auto" : `${controlWidth}px` } as CSSProperties) : undefined}
        >
          <div className="g-field__row">
            {text}
            {control}
          </div>
        </div>
      ) : (
        <div className={cx("g-field", "g-field--stack", error && "is-invalid", className)}>
          {text}
          {control}
          {(error || help) && <p id={helpId} className={cx("g-field__help", error && "is-error")}>{error ?? help}</p>}
        </div>
      )}
    </FieldContext.Provider>
  );
}

/**
 * Development only. A text field's edge is about 2.5:1 (the recorded contrast exception in
 * docs/design-system.md), so an empty field must never rely on its edge alone: it needs a
 * placeholder showing an example value. Warns once per field when one is empty without one.
 */
function useEmptyFieldCheck(controlId: string) {
  const warned = useRef(false);
  useEffect(() => {
    if (!import.meta.env.DEV || warned.current) return;
    const el = document.getElementById(controlId);
    if (!(el instanceof HTMLInputElement || el instanceof HTMLTextAreaElement)) return;
    if (el instanceof HTMLInputElement && !TEXT_TYPES.has(el.type)) return;
    if (el.value === "" && !el.placeholder && !el.disabled) {
      warned.current = true;
      console.warn(`[Field] "${el.labels?.[0]?.textContent ?? controlId}" is empty and has no placeholder. Add an example value as the placeholder (design-system.md, Field).`);
    }
  });
}
const TEXT_TYPES = new Set(["text", "password", "email", "url", "search", "tel", "number"]);

type Size = "sm" | "md" | "lg";

export interface InputProps extends Omit<ComponentPropsWithRef<"input">, "size"> {
  size?: Size;
  leading?: ReactNode;
  trailing?: ReactNode;
  mono?: boolean;
  invalid?: boolean;
}

export function Input({ size = "md", leading, trailing, mono, invalid, className, ...rest }: InputProps) {
  const a = useFieldControl(rest);
  return (
    <span className={cx("g-input", `g-input--${size}`, mono && "g-input--mono", (invalid || a["aria-invalid"]) && "is-invalid", rest.disabled && "is-disabled", className)}>
      {leading && <span className="g-input__adorn">{leading}</span>}
      <input {...rest} {...a} aria-invalid={invalid || a["aria-invalid"]} />
      {trailing && <span className="g-input__adorn g-input__adorn--end">{trailing}</span>}
    </span>
  );
}

export interface TextareaProps extends ComponentPropsWithRef<"textarea"> {
  mono?: boolean;
  invalid?: boolean;
}

export function Textarea({ mono, invalid, className, rows = 4, ...rest }: TextareaProps) {
  const a = useFieldControl(rest);
  return (
    <textarea
      rows={rows}
      className={cx("g-textarea", mono && "g-input--mono", (invalid || a["aria-invalid"]) && "is-invalid", className)}
      {...rest}
      {...a}
    />
  );
}

export interface SelectOption {
  value: string;
  label: string;
  disabled?: boolean;
}

export interface SelectProps extends Omit<ComponentPropsWithRef<"select">, "size"> {
  size?: Size;
  /** Either pass options or <option> children. */
  options?: SelectOption[];
  invalid?: boolean;
}

/** A native <select>, styled. Native keeps keyboard, screen-reader and WebKitGTK behavior right. */
export function Select({ size = "md", options, invalid, className, children, ...rest }: SelectProps) {
  const a = useFieldControl(rest);
  return (
    <span className={cx("g-select", `g-input--${size}`, (invalid || a["aria-invalid"]) && "is-invalid", rest.disabled && "is-disabled", className)}>
      <select {...rest} {...a}>
        {options
          ? options.map((o) => (
              <option key={o.value} value={o.value} disabled={o.disabled}>
                {o.label}
              </option>
            ))
          : children}
      </select>
      <ChevronDown className="g-select__chevron" size={14} aria-hidden />
    </span>
  );
}
