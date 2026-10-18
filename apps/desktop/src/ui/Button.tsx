import { type ComponentPropsWithRef, createContext, type ReactElement, type ReactNode, useContext } from "react";

import { cx } from "./cx";
import { Tooltip } from "./Tooltip";

export type ButtonVariant = "primary" | "secondary" | "ghost" | "danger" | "danger-ghost";
export type ButtonSize = "sm" | "md" | "lg";

/**
 * The default size for buttons that don't set one. The compact page bar sets "sm" so a
 * page's header actions shrink with it; an explicit `size` always wins.
 */
export const ButtonSizeContext = createContext<ButtonSize | null>(null);

/** Link form: pass `href` and the button renders an <a> with the same look and states. */
interface LinkProps {
  href?: string;
  target?: string;
  rel?: string;
  download?: string | boolean;
}

export interface ButtonProps extends ComponentPropsWithRef<"button">, LinkProps {
  variant?: ButtonVariant;
  size?: ButtonSize;
  /**
   * Shows a spinner, sets aria-busy and blocks clicks, without changing the width: the
   * spinner replaces the leading icon in the same box, or sits over the hidden label when
   * there is none. Keep the label the same while loading; say "Saving" with a Toast or
   * Notice, never by swapping the children.
   */
  loading?: boolean;
  leadingIcon?: ReactNode;
  trailingIcon?: ReactNode;
  fullWidth?: boolean;
}

const ICON_PX: Record<ButtonSize, number> = { sm: 14, md: 16, lg: 16 };

/**
 * The one button. `primary` once per view at most (the focal action); `secondary` for
 * everything else that matters; `ghost` for tertiary and toolbar actions; `danger` only
 * as the confirm of a destructive dialog; `danger-ghost` for a destructive entry point.
 * With `href` it is a link that looks like a button ("Open on GitHub").
 */
export function Button({
  variant = "secondary",
  size: sizeProp,
  loading = false,
  leadingIcon,
  trailingIcon,
  fullWidth,
  className,
  children,
  disabled,
  type = "button",
  href,
  target,
  rel,
  download,
  ...rest
}: ButtonProps) {
  const ctx = useContext(ButtonSizeContext);
  const size = sizeProp ?? ctx ?? "md";
  const cls = cx(
    "g-btn",
    `g-btn--${variant}`,
    `g-btn--${size}`,
    fullWidth && "g-btn--full",
    leadingIcon != null && "has-leading",
    loading && "is-loading",
    className,
  );
  const body = (
    <>
      {loading && leadingIcon != null ? <Spinner size={ICON_PX[size]} /> : leadingIcon}
      {children != null && <span className="g-btn__label">{children}</span>}
      {trailingIcon}
      {loading && leadingIcon == null && <Spinner size={ICON_PX[size]} className="g-btn__overlay" />}
    </>
  );
  if (href !== undefined) {
    const off = disabled || loading;
    return (
      <a
        className={cx(cls, off && "is-disabled")}
        href={off ? undefined : href}
        target={target}
        rel={rel ?? (target === "_blank" ? "noreferrer" : undefined)}
        download={download}
        aria-disabled={off || undefined}
        aria-busy={loading || undefined}
        {...(rest as ComponentPropsWithRef<"a">)}
      >
        {body}
      </a>
    );
  }
  return (
    // Loading is aria-disabled, not disabled: a disabled button drops keyboard focus to the
    // page body, so pressing Enter on Save would lose your place. Clicks are ignored instead.
    <button
      type={type}
      className={cls}
      disabled={disabled}
      aria-disabled={loading || undefined}
      aria-busy={loading || undefined}
      {...rest}
      onClick={(e) => {
        if (loading) return e.preventDefault();
        rest.onClick?.(e);
      }}
    >
      {body}
    </button>
  );
}

export interface IconButtonProps extends Omit<ComponentPropsWithRef<"button">, "children">, LinkProps {
  /** Required: the accessible name, also shown as the tooltip. */
  label: string;
  icon: ReactElement;
  variant?: "ghost" | "secondary" | "primary";
  size?: "sm" | "md" | "lg";
  /** For toggle buttons: sets aria-pressed and the selected look. */
  pressed?: boolean;
  loading?: boolean;
  /** Set false to drop the tooltip (e.g. when the label is visible next to it). */
  tooltip?: boolean;
  tooltipSide?: "top" | "bottom" | "left" | "right";
}

export function IconButton({
  label,
  icon,
  variant = "ghost",
  size: sizeProp,
  pressed,
  loading,
  tooltip = true,
  tooltipSide = "top",
  className,
  disabled,
  type = "button",
  href,
  target,
  rel,
  download,
  ...rest
}: IconButtonProps) {
  const ctx = useContext(ButtonSizeContext);
  const size = sizeProp ?? ctx ?? "md";
  const cls = cx("g-icon-btn", `g-btn--${variant}`, `g-icon-btn--${size}`, pressed && "is-pressed", loading && "is-loading", className);
  const glyph = loading ? <Spinner size={size === "sm" ? 14 : 16} /> : icon;
  const off = disabled || loading;
  const button =
    href !== undefined ? (
      <a
        aria-label={label}
        className={cx(cls, off && "is-disabled")}
        href={off ? undefined : href}
        target={target}
        rel={rel ?? (target === "_blank" ? "noreferrer" : undefined)}
        download={download}
        aria-disabled={off || undefined}
        aria-busy={loading || undefined}
        {...(rest as ComponentPropsWithRef<"a">)}
      >
        {glyph}
      </a>
    ) : (
      <button
        type={type}
        aria-label={label}
        aria-pressed={pressed}
        aria-busy={loading || undefined}
        aria-disabled={loading || undefined}
        disabled={disabled}
        className={cls}
        {...rest}
        onClick={(e) => {
          if (loading) return e.preventDefault();
          rest.onClick?.(e);
        }}
      >
        {glyph}
      </button>
    );
  return tooltip ? (
    <Tooltip content={label} side={tooltipSide}>
      {button}
    </Tooltip>
  ) : (
    button
  );
}

/** Indeterminate spinner. Decorative unless given a label. */
export function Spinner({ size = 16, label, className }: { size?: number; label?: string; className?: string }) {
  return (
    <svg
      className={cx("g-spinner", className)}
      width={size}
      height={size}
      viewBox="0 0 16 16"
      fill="none"
      role={label ? "status" : undefined}
      aria-label={label}
      aria-hidden={label ? undefined : true}
    >
      <circle cx="8" cy="8" r="6.25" stroke="currentColor" strokeOpacity="0.22" strokeWidth="1.5" />
      <path d="M14.25 8A6.25 6.25 0 0 0 8 1.75" stroke="currentColor" strokeWidth="1.5" strokeLinecap="round" />
    </svg>
  );
}
