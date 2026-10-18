/**
 * Owner: policy. The policy file as text, with line numbers so the daemon's parse errors
 * ("at line 4") point somewhere. No wrapping, so a number always sits beside its line.
 */
import { type Ref, useId } from "react";

import { cx } from "../../ui";

export interface TomlEditorProps {
  value: string;
  onChange: (text: string) => void;
  /** 1-based line the daemon complained about. */
  errorLine?: number | null;
  disabled?: boolean;
  label: string;
  describedBy?: string;
  placeholder?: string;
  inputRef?: Ref<HTMLTextAreaElement>;
}

export function TomlEditor({ value, onChange, errorLine, disabled, label, describedBy, placeholder, inputRef }: TomlEditorProps) {
  const id = useId();
  const lines = Math.max(1, value.split("\n").length);
  const rows = Math.max(18, lines + 1);
  return (
    <div className={cx("policy-toml", errorLine && "is-invalid", disabled && "is-disabled")}>
      <div className="policy-toml__gutter" aria-hidden>
        {Array.from({ length: rows }, (_, i) => (
          <span key={i} className={cx(i + 1 === errorLine && "is-error", i >= lines && "is-past")}>
            {i + 1}
          </span>
        ))}
      </div>
      {errorLine != null && errorLine <= rows && (
        <span className="policy-toml__band" style={{ top: `calc(var(--policy-toml-pad) + ${errorLine - 1} * var(--leading-md))` }} aria-hidden />
      )}
      <textarea
        ref={inputRef}
        id={id}
        className="policy-toml__input"
        value={value}
        rows={rows}
        wrap="off"
        spellCheck={false}
        autoCapitalize="off"
        autoComplete="off"
        autoCorrect="off"
        disabled={disabled}
        aria-label={label}
        aria-describedby={describedBy}
        aria-invalid={errorLine ? true : undefined}
        placeholder={placeholder}
        onChange={(e) => onChange(e.target.value)}
      />
    </div>
  );
}
