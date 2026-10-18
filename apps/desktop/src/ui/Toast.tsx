import { CircleCheck, CircleX, Info, TriangleAlert, X } from "lucide-react";
import { createContext, type ReactNode, useCallback, useContext, useEffect, useMemo, useRef, useState } from "react";

import { cx } from "./cx";

export interface ToastInput {
  title: ReactNode;
  description?: ReactNode;
  tone?: "neutral" | "ok" | "warn" | "danger" | "info";
  /** One short action, e.g. Undo. */
  action?: { label: string; onClick: () => void };
  /** ms; default 5000, 0 keeps it until dismissed. Danger toasts default to 8000. */
  duration?: number;
}
interface ToastItem extends ToastInput {
  id: number;
}

const ToastCtx = createContext<((t: ToastInput) => number) | null>(null);

/** Transient confirmation of something that just happened. Errors that block work go in a Notice instead. */
export function useToast() {
  const push = useContext(ToastCtx);
  if (!push) throw new Error("useToast must be used inside <ToastProvider>");
  return push;
}

const ICON = { neutral: Info, info: Info, ok: CircleCheck, warn: TriangleAlert, danger: CircleX };

export function ToastProvider({ children }: { children: ReactNode }) {
  const [items, setItems] = useState<ToastItem[]>([]);
  const next = useRef(1);
  const dismiss = useCallback((id: number) => setItems((xs) => xs.filter((x) => x.id !== id)), []);
  const push = useCallback((t: ToastInput) => {
    const id = next.current++;
    setItems((xs) => [...xs.slice(-3), { ...t, id }]);
    return id;
  }, []);
  const value = useMemo(() => push, [push]);
  return (
    <ToastCtx.Provider value={value}>
      {children}
      <div className="g-toaster" role="region" aria-label="Notifications" aria-live="polite">
        {items.map((t) => (
          <Toast key={t.id} item={t} onDismiss={() => dismiss(t.id)} />
        ))}
      </div>
    </ToastCtx.Provider>
  );
}

function Toast({ item, onDismiss }: { item: ToastItem; onDismiss: () => void }) {
  const tone = item.tone ?? "neutral";
  const Icon = ICON[tone];
  const [paused, setPaused] = useState(false);
  const duration = item.duration ?? (tone === "danger" ? 8000 : 5000);
  useEffect(() => {
    if (!duration || paused) return;
    const t = setTimeout(onDismiss, duration);
    return () => clearTimeout(t);
  }, [duration, paused, onDismiss]);
  return (
    <div
      className={cx("g-toast", `g-toast--${tone}`)}
      role={tone === "danger" ? "alert" : "status"}
      onPointerEnter={() => setPaused(true)}
      onPointerLeave={() => setPaused(false)}
      onFocus={() => setPaused(true)}
      onBlur={() => setPaused(false)}
    >
      <Icon size={16} className="g-toast__icon" aria-hidden />
      <div className="g-toast__body">
        <p className="g-toast__title">{item.title}</p>
        {item.description && <p className="g-toast__description">{item.description}</p>}
      </div>
      {item.action && (
        <button
          type="button"
          className="g-toast__action"
          onClick={() => {
            item.action!.onClick();
            onDismiss();
          }}
        >
          {item.action.label}
        </button>
      )}
      <button type="button" className="g-toast__close" aria-label="Dismiss" onClick={onDismiss}>
        <X size={14} aria-hidden />
      </button>
    </div>
  );
}
