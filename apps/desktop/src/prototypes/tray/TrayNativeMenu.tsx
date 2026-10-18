/**
 * Owner: tray. The menu Ubuntu actually shows when you click Ghost's top-bar icon.
 *
 * An AppIndicator (Tauri's Linux tray) can't open a web view under the icon: a click opens
 * a native dbusmenu that GNOME Shell draws itself. It can show only text items (enabled or
 * not), checkbox items and separators, in the shell's font and colors. So this component
 * draws nothing a dbusmenu can't: no icons, badges, colors or progress. Today's lib.rs has
 * Open Ghost, Wake and Quit; this is the full menu Ghost should ship, built from the same
 * state the popover reads, with "Show status…" opening the popover as its own window.
 */
import { useGhost } from "@ghost/ui";
import { forwardRef, useEffect, useImperativeHandle, useRef } from "react";

import { inTauri, startDaemon } from "../../transport";
import { cx, useLive } from "../../ui";
import { canUndo, shortChange, useLastChange } from "./data";
import { actionables, roveKey } from "./focus";
import { type TrayState, lowerFirst } from "./model";

export interface TrayNativeMenuProps {
  state: TrayState;
  onMutedChange: (muted: boolean) => void;
  /** Close the menu; `refocus` hands focus back to the indicator (keyboard use). */
  onClose: (refocus: boolean) => void;
  /** "Show status…": open the popover window. An error from a menu action goes with it. */
  onShowStatus: (error?: string) => void;
  openHref: string;
  onQuit: () => void;
  /** Opened from the keyboard: focus the first item, as GNOME does. */
  autoFocus: boolean;
  id?: string;
}

type Item =
  | { kind: "label"; text: string }
  | { kind: "separator" }
  | { kind: "item"; text: string; run: () => Promise<unknown> | void; disabled?: boolean; href?: string }
  | { kind: "check"; text: string; checked: boolean; run: () => void; disabled?: boolean };

export const TrayNativeMenu = forwardRef<HTMLDivElement, TrayNativeMenuProps>(function TrayNativeMenu(
  { state, onMutedChange, onClose, onShowStatus, openHref, onQuit, autoFocus, id },
  ref,
) {
  const { client, status, pending } = useGhost();
  const { connection, online, loadingModels } = useLive();
  const { entry: last } = useLastChange();
  const root = useRef<HTMLDivElement>(null);
  useImperativeHandle(ref, () => root.current as HTMLDivElement);

  useEffect(() => {
    if (autoFocus) actionables(root.current)[0]?.focus();
    else root.current?.focus();
  }, [autoFocus]);

  // Every activation closes the menu, as dbusmenu does. A failure has nowhere to show in a
  // native menu, so it opens the status window with the error in it.
  const activate = (it: Item, keyboard: boolean) => {
    if (it.kind !== "item" && it.kind !== "check") return;
    onClose(keyboard);
    const r = it.run();
    if (r && typeof (r as Promise<unknown>).catch === "function") {
      (r as Promise<unknown>).catch((e: unknown) => onShowStatus(e instanceof Error ? e.message : String(e)));
    }
  };

  const items: Item[] = [];
  const statusLine =
    connection === "offline" || connection === "connecting"
      ? state.title
      : state.inSession
        ? `${state.title} · ${lowerFirst(state.where)}`
        : state.muted || loadingModels
          ? `${state.title} · ${lowerFirst(state.sub)}`
          : state.title;
  items.push({ kind: "label", text: statusLine });

  if (pending) {
    // One group: a disabled heading, then Confirm naming exactly what it approves (a bare
    // "Confirm" is easy to hit without reading the line above), then Cancel. The click is
    // confirmation evidence, exactly as the popover's Confirm (ADR 0013). The whole card,
    // with its reason and clock, is one item down: Show status…
    items.push({ kind: "separator" });
    items.push({ kind: "label", text: "Waiting for your okay" });
    items.push({
      kind: "item",
      text: `Confirm: ${shortChange(pending.summary, pending.target)}`,
      run: () => client.confirm(),
      disabled: !online,
    });
    items.push({ kind: "item", text: "Cancel", run: () => client.cancel(), disabled: !online });
  }

  items.push({ kind: "separator" });
  if (connection !== "offline" && connection !== "connecting") {
    // No mute API yet: the control exists only in the mock layer (the stage's legend says so);
    // the string itself ships verbatim through dbusmenu, so it carries no mark.
    items.push({
      kind: "check",
      text: "Mute microphone",
      checked: state.muted,
      run: () => onMutedChange(!state.muted),
      disabled: !online,
    });
  }
  if (connection === "offline" && inTauri) {
    items.push({ kind: "item", text: "Start Ghost", run: startDaemon });
  } else if (state.inSession) {
    items.push({ kind: "item", text: "Go to sleep", run: () => client.sleep(), disabled: !online });
  } else if (!state.muted) {
    // Muted: no Wake at all, as in the status window; the status line says why.
    items.push({
      kind: "item",
      text: "Wake",
      run: () => client.wake(),
      disabled: !online || !status?.models_ready,
    });
  }
  if (canUndo(last) && last) {
    items.push({ kind: "item", text: `Undo “${shortChange(last.summary, last.target)}”`, run: () => client.undo(), disabled: !online });
  }

  items.push({ kind: "separator" });
  items.push({ kind: "item", text: "Show status…", run: () => onShowStatus() });
  items.push({ kind: "item", text: "Open Ghost", run: () => onClose(false), href: openHref });
  items.push({ kind: "item", text: "Quit", run: onQuit });

  return (
    <div
      ref={root}
      id={id}
      className="tray-native"
      role="menu"
      aria-label="Ghost"
      tabIndex={-1}
      onKeyDown={(e) => {
        if (e.key === "Tab") {
          e.preventDefault();
          onClose(true);
          return;
        }
        roveKey(e, root.current);
      }}
    >
      {items.map((it, i) => {
        if (it.kind === "separator") return <div key={i} className="tray-native__sep" role="separator" />;
        if (it.kind === "label")
          return (
            <div key={i} className="tray-native__item is-label" role="menuitem" aria-disabled="true">
              {it.text}
            </div>
          );
        const kb = (e: React.MouseEvent) => e.detail === 0;
        if (it.kind === "check")
          return (
            <button
              key={i}
              type="button"
              role="menuitemcheckbox"
              aria-checked={it.checked}
              className="tray-native__item"
              disabled={it.disabled}
              tabIndex={-1}
              onClick={(e) => activate(it, kb(e))}
            >
              <span className="tray-native__label">{it.text}</span>
              <span className={cx("tray-native__check", it.checked && "is-on")} aria-hidden />
            </button>
          );
        if (it.href && !it.disabled)
          return (
            <a key={i} role="menuitem" className="tray-native__item" href={it.href} tabIndex={-1} onClick={() => onClose(false)}>
              <span className="tray-native__label">{it.text}</span>
            </a>
          );
        return (
          <button
            key={i}
            type="button"
            role="menuitem"
            className="tray-native__item"
            disabled={it.disabled}
            tabIndex={-1}
            onClick={(e) => activate(it, kb(e))}
          >
            <span className="tray-native__label">{it.text}</span>
          </button>
        );
      })}
    </div>
  );
});
