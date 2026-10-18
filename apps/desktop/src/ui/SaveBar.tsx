import { Check, Copy, RotateCw, X } from "lucide-react";
import { type ReactNode, type Ref, useEffect, useState } from "react";

import { Button, IconButton } from "./Button";
import { cx } from "./cx";
import { Kbd } from "./Display";
import { Tooltip } from "./Tooltip";

export type SaveBarVariant = "dirty" | "error" | "offline" | "restart";

export interface SaveBarProps {
  /**
   * dirty: unsaved changes, Discard and Save. error: the last save failed (red edge, --danger
   * dot), Discard and Save stay. offline: Ghost isn't answering, Save is off and the words say
   * the changes wait here. restart: a save that owes a restart, with the command and Dismiss.
   */
  variant?: SaveBarVariant;
  /** The lead, in `--text-1`: “3 unsaved changes”, “Couldn’t save”, “Restart Ghost to apply”. */
  title: string;
  /** After the middle dot, lowercase: what changed, the fault, “they wait here until Ghost is back”. */
  detail?: ReactNode;
  /** A second, smaller line inside the bar: a save error’s reason. */
  description?: ReactNode;
  /** Full text for hover and screen readers when `detail` may truncate. */
  hover?: string;
  /** The mono detail slot: a shell command (`systemctl --user restart ghostd`), copied with one click. */
  command?: string;
  onSave?: () => void;
  onDiscard?: () => void;
  /** restart: the Dismiss button. */
  onDismiss?: () => void;
  /** Save is saving: spinner on Save, Discard off. */
  busy?: boolean;
  saveLabel?: string;
  discardLabel?: string;
  /** Replaces Discard and Save (a custom action set). */
  actions?: ReactNode;
  /** Rendered above the bar, in the dock: a Notice the bar can't carry itself. */
  notice?: ReactNode;
  saveRef?: Ref<HTMLButtonElement>;
  /** The restart variant's Copy button, so focus can land on it when it replaces the dirty bar. */
  copyRef?: Ref<HTMLButtonElement>;
  className?: string;
}

/** Copies `text`, then reads “Copied” for a moment. */
export function useCopy(): [boolean, (text: string) => void] {
  const [copied, setCopied] = useState(false);
  useEffect(() => {
    if (!copied) return;
    const t = setTimeout(() => setCopied(false), 1600);
    return () => clearTimeout(t);
  }, [copied]);
  return [copied, (text) => void navigator.clipboard?.writeText(text).then(() => setCopied(true), () => {})];
}

/**
 * The one control that commits a change, in one shape on every screen that has it: a 44px
 * pill in the Page dock, at least 480 wide and growing to its content up to the page width.
 * A dot and one line (lead, dot, detail), the shortcut cap while Save can run, then ghost
 * Discard and filled Save 12px apart. Save is the page's primary button; the shortcut is
 * Ctrl S, bound by the screen, shown here. The dirty dot is neutral: amber means only
 * “waiting for your okay”, and the accent marks the primary action, which is Save itself.
 *
 * Toast after a save: title “Saved”, body says when it takes effect (“Ghost uses the new
 * levels from its next change.”), Undo where possible. The restart variant is the only
 * exception: nothing is toasted, the bar says what is left to do.
 */
export function SaveBar({
  variant = "dirty",
  title,
  detail,
  description,
  hover,
  command,
  onSave,
  onDiscard,
  onDismiss,
  busy = false,
  saveLabel = "Save",
  discardLabel = "Discard",
  actions,
  notice,
  saveRef,
  copyRef,
  className,
}: SaveBarProps) {
  const [copied, copy] = useCopy();
  const restart = variant === "restart";
  const offline = variant === "offline";
  const fullText = hover ?? (typeof detail === "string" ? `${title}. ${detail.charAt(0).toUpperCase()}${detail.slice(1)}.` : undefined);
  return (
    <div className={cx("g-savebar-dock", className)}>
      {notice}
      <div
        className={cx("g-savebar", `g-savebar--${variant}`, description && "has-description")}
        role={restart ? "status" : "region"}
        aria-label={restart ? undefined : "Unsaved changes"}
      >
        {restart ? (
          <span className="g-savebar__icon" aria-hidden>
            <RotateCw />
          </span>
        ) : (
          <span className="g-savebar__dot" aria-hidden />
        )}
        <p className="g-savebar__text" title={fullText} aria-live={restart ? undefined : "polite"}>
          <span className="g-savebar__lead">{title}</span>
          {detail && (
            <>
              <span className="g-savebar__sep" aria-hidden>
                ·
              </span>
              <span className="sr-only">: </span>
              <span className="g-savebar__detail" aria-hidden={hover ? true : undefined}>
                {detail}
              </span>
              {hover && <span className="sr-only">{hover}</span>}
            </>
          )}
        </p>
        {description && (
          <p className="g-savebar__description" aria-live="polite">
            {description}
          </p>
        )}
        {command && (
          <span className="g-savebar__command">
            <code>{command}</code>
            <IconButton ref={copyRef} size="sm" label={copied ? "Copied" : "Copy command"} icon={copied ? <Check /> : <Copy />} onClick={() => copy(command)} />
          </span>
        )}
        {/* The caps go while the shortcut can't run: offline, or already saving. */}
        {!restart && !command && !offline && !busy && (
          <span className="g-savebar__hint" aria-hidden>
            <Kbd shortcut="mod+s" />
          </span>
        )}
        <div className="g-savebar__actions">
          {actions ??
            (restart ? (
              onDismiss && <IconButton size="sm" variant="ghost" label="Dismiss" icon={<X />} onClick={onDismiss} />
            ) : (
              <>
                <Button variant="ghost" onClick={onDiscard} disabled={busy}>
                  {discardLabel}
                </Button>
                <Tooltip content={saveLabel} shortcut="mod+s">
                  <Button ref={saveRef} variant="primary" loading={busy} disabled={offline} onClick={onSave}>
                    {saveLabel}
                  </Button>
                </Tooltip>
              </>
            ))}
        </div>
      </div>
    </div>
  );
}
