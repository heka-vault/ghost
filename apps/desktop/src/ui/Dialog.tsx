import { X } from "lucide-react";
import { type KeyboardEvent, type ReactNode, type RefObject, useCallback, useEffect, useId, useRef, useState } from "react";

import { Button } from "./Button";
import { cx } from "./cx";

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  title: ReactNode;
  description?: ReactNode;
  children?: ReactNode;
  /** Buttons, right-aligned. Put the primary action last. */
  footer?: ReactNode;
  size?: "sm" | "md" | "lg";
  /** Escape and backdrop clicks close it (default true). Set false while saving. */
  dismissible?: boolean;
  /** Hide the header's close button (ConfirmDialog does: Cancel is the way out, and gets first focus). */
  hideClose?: boolean;
  /** Where focus lands on open: a selector inside the dialog, or a ref. Default: the first control. */
  initialFocus?: string | RefObject<HTMLElement | null>;
  className?: string;
}

const FOCUSABLE =
  'a[href], button:not([disabled]), input:not([disabled]):not([type="hidden"]), select:not([disabled]), textarea:not([disabled]), [tabindex]:not([tabindex="-1"]), [contenteditable="true"]';

/** Tab and Shift+Tab wrap between the first and last focusable control. */
function wrapTab(e: KeyboardEvent<HTMLDialogElement>) {
  if (e.key !== "Tab" || e.altKey || e.ctrlKey || e.metaKey) return;
  const all = Array.from(e.currentTarget.querySelectorAll<HTMLElement>(FOCUSABLE)).filter(
    (el) => !el.closest("[inert]") && el.getClientRects().length > 0,
  );
  if (all.length === 0) {
    e.preventDefault();
    return;
  }
  const first = all[0];
  const last = all[all.length - 1];
  const at = document.activeElement;
  if (e.shiftKey && (at === first || !e.currentTarget.contains(at))) {
    e.preventDefault();
    last.focus();
  } else if (!e.shiftKey && (at === last || !e.currentTarget.contains(at))) {
    e.preventDefault();
    first.focus();
  }
}

/**
 * A modal on the native <dialog> element: the page behind is inert, focus returns to the
 * opener on close, Escape closes. The browser lets Tab leave a modal dialog for its own
 * chrome, so Tab and Shift+Tab are wrapped here: focus cycles inside the dialog. Use sparingly: for destructive confirmation
 * and short focused tasks. Never for information that could sit inline.
 */
export function Dialog({ open, onClose, title, description, children, footer, size = "sm", dismissible = true, hideClose, initialFocus, className }: DialogProps) {
  const ref = useRef<HTMLDialogElement>(null);
  const titleId = useId();
  const descId = useId();

  useEffect(() => {
    const d = ref.current;
    if (!d) return;
    if (open && !d.open) {
      d.showModal();
      const target = typeof initialFocus === "string" ? d.querySelector<HTMLElement>(initialFocus) : initialFocus?.current;
      target?.focus({ preventScroll: true });
    }
    if (!open && d.open) d.close();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  return (
    <dialog
      ref={ref}
      className={cx("g-dialog", `g-dialog--${size}`, className)}
      aria-labelledby={titleId}
      aria-describedby={description ? descId : undefined}
      onCancel={(e) => {
        e.preventDefault();
        if (dismissible) onClose();
      }}
      onKeyDown={wrapTab}
      onMouseDown={(e) => {
        if (dismissible && e.target === ref.current) onClose();
      }}
    >
      {open && (
        <div className="g-dialog__panel">
          <header className="g-dialog__header">
            <h2 id={titleId} className="g-dialog__title">{title}</h2>
            {dismissible && !hideClose && (
              <button type="button" className="g-dialog__close" aria-label="Close" onClick={onClose}>
                <X size={16} aria-hidden />
              </button>
            )}
          </header>
          {description && <p id={descId} className="g-dialog__description">{description}</p>}
          {children && <div className="g-dialog__body">{children}</div>}
          {footer && <footer className="g-dialog__footer">{footer}</footer>}
        </div>
      )}
    </dialog>
  );
}

export interface ConfirmOptions {
  title: ReactNode;
  description?: ReactNode;
  confirmLabel?: string;
  cancelLabel?: string;
  tone?: "default" | "danger";
}

export interface ConfirmDialogProps extends ConfirmOptions {
  open: boolean;
  /** May return a promise; the dialog shows loading and stays open until it settles. */
  onConfirm: () => unknown;
  onCancel: () => void;
}

/** Two-button confirmation. The confirm label names the action ("Delete Accounting"), never "OK". */
export function ConfirmDialog({ open, onConfirm, onCancel, title, description, confirmLabel = "Confirm", cancelLabel = "Cancel", tone = "default" }: ConfirmDialogProps) {
  const [busy, setBusy] = useState(false);
  const run = async () => {
    setBusy(true);
    try {
      await onConfirm();
    } finally {
      setBusy(false);
    }
  };
  return (
    <Dialog
      open={open}
      onClose={onCancel}
      title={title}
      description={description}
      dismissible={!busy}
      hideClose
      footer={
        <>
          <Button variant="ghost" onClick={onCancel} disabled={busy} autoFocus={tone === "danger"}>
            {cancelLabel}
          </Button>
          <Button variant={tone === "danger" ? "danger" : "primary"} loading={busy} onClick={run} autoFocus={tone !== "danger"}>
            {confirmLabel}
          </Button>
        </>
      }
    />
  );
}

/**
 * Promise-style confirmation, the replacement for window.confirm:
 *   const { confirm, dialog } = useConfirm();
 *   if (await confirm({ title: "Delete Accounting?", tone: "danger", confirmLabel: "Delete" })) ...
 *   return <>{...}{dialog}</>;
 */
export function useConfirm() {
  const [state, setState] = useState<(ConfirmOptions & { resolve: (ok: boolean) => void }) | null>(null);
  const confirm = useCallback(
    (opts: ConfirmOptions) => new Promise<boolean>((resolve) => setState({ ...opts, resolve })),
    [],
  );
  const dialog = (
    <ConfirmDialog
      open={!!state}
      title={state?.title ?? ""}
      description={state?.description}
      confirmLabel={state?.confirmLabel}
      cancelLabel={state?.cancelLabel}
      tone={state?.tone}
      onConfirm={() => {
        state?.resolve(true);
        setState(null);
      }}
      onCancel={() => {
        state?.resolve(false);
        setState(null);
      }}
    />
  );
  return { confirm, dialog };
}
