/**
 * Owner: tray. PROTOTYPE: Ghost in the Ubuntu top bar. Rendered without the app shell, at
 * real size on a stand-in desktop: the top bar with Ghost's indicator, what opens from it,
 * and the listening HUD at the bottom of the screen.
 *
 * Two surfaces open from the indicator:
 *  - the native menu (`&native=1`): what an AppIndicator really shows on Ubuntu, a dbusmenu
 *    drawn by GNOME Shell. Its "Show status…" item opens
 *  - the status popover: Ghost's own window, with the waiting change, mute, the last change.
 *
 * States by URL (any mock scenario works; these are the designed ones):
 *   ?mock=passive#/tray            waiting for “Hey Ghost”
 *   ?mock=session#/tray            listening on the Accounting line, HUD up
 *   ?mock=thinking#/tray           thinking
 *   ?mock=speaking#/tray           speaking, the HUD captions what Ghost is saying
 *   ?mock=pending#/tray            a change waiting for the operator's okay
 *   ?mock=passive&muted=1#/tray    microphone muted (prototype: no API yet)
 *   ?mock=starting#/tray           loading voice models
 *   ?mock=disconnected#/tray       the service is stopped
 *   ?mock=reconnecting#/tray       the daemon dropped mid-session (after 1.5 s); the HUD stays
 *   ?mock=firstrun#/tray           no model key yet
 *   ?mock=expiring#/tray           the change lapses at 8 s and fades out as Expired
 * Add &native=1 for the native menu, &menu=closed to start with nothing open.
 */
import { useGhost } from "@ghost/ui";
import { BatteryFull, MicOff, Volume2, Wifi } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { scenarioHref, useDesign, useFixture } from "../../design/context";
import type { TrayFixture } from "../../design/mock/fixtures/tray";
import { Badge, Button, GhostGlyph, StatusDot, cx, quoted, useLive } from "../../ui";
import { useLines } from "./data";
import { actionables, useFocusKeeper, usingKeyboard } from "./focus";
import { stateName, trayState } from "./model";
import { TrayHud } from "./TrayHud";
import { TrayNativeMenu } from "./TrayNativeMenu";
import { TrayPopover } from "./TrayPopover";
import "./tray.css";

const SURFACE_ID = "tray-surface";

const STATES: { label: string; mock: string; muted?: boolean }[] = [
  { label: "Passive", mock: "passive" },
  { label: "Listening", mock: "session" },
  { label: "Thinking", mock: "thinking" },
  { label: "Speaking", mock: "speaking" },
  { label: "Waiting for okay", mock: "pending" },
  { label: "Muted", mock: "passive", muted: true },
  { label: "Starting", mock: "starting" },
  { label: "Not running", mock: "disconnected" },
  { label: "Reconnecting", mock: "reconnecting" },
  { label: "Not set up", mock: "firstrun" },
];

function param(name: string): string | null {
  return new URLSearchParams(window.location.search).get(name);
}

/** Rewrite one query parameter in place (no navigation), so each state stays one link away. */
function setParam(name: string, value: string | null) {
  const q = new URLSearchParams(window.location.search);
  if (value == null) q.delete(name);
  else q.set(name, value);
  window.history.replaceState(null, "", `${window.location.pathname}?${q.toString()}${window.location.hash}`);
}

export default function TrayPrototype(_: RouteProps) {
  const { status, pending } = useGhost();
  const live = useLive();
  const design = useDesign();
  const data = useFixture<TrayFixture>("tray");
  const lines = useLines();

  const [muted, setMuted] = useState(() => (param("muted") != null ? param("muted") === "1" : data?.muted ?? false));
  const [native] = useState(() => param("native") === "1");
  // What is open under the indicator: the native menu, the status popover, or nothing.
  const [open, setOpen] = useState<null | "menu" | "status">(() =>
    param("menu") === "closed" ? null : param("native") === "1" ? "menu" : "status",
  );
  const [autoFocus, setAutoFocus] = useState(false);
  const [statusError, setStatusError] = useState<string | null>(null);
  const [quit, setQuit] = useState(false);
  const stage = useRef<HTMLDivElement>(null);
  const surface = useRef<HTMLElement | null>(null);
  const indicator = useRef<HTMLButtonElement>(null);
  const reopen = useRef<HTMLButtonElement>(null);
  const stageFocus = useFocusKeeper(stage, () => null);

  // Quit takes the menu and the icon away; Open the app brings the icon back. Either way the
  // control that had focus is gone, so name where it goes (keyboard users only).
  const quitApp = useCallback(() => {
    stageFocus.request(() => reopen.current);
    setOpen(null);
    setQuit(true);
  }, [stageFocus]);
  const reopenApp = useCallback(() => {
    stageFocus.request(() => indicator.current);
    setQuit(false);
  }, [stageFocus]);

  const state = trayState({
    connection: live.connection,
    phase: live.phase,
    status,
    muted,
    setupNeeded: live.setupNeeded,
    lineNameOf: (slug) => lines.find((l) => l.slug === slug)?.name,
  });

  const changeMuted = useCallback((m: boolean) => {
    setMuted(m);
    setParam("muted", m ? "1" : null);
  }, []);

  const close = useCallback((refocus: boolean) => {
    setOpen(null);
    setAutoFocus(false);
    if (refocus) indicator.current?.focus();
  }, []);

  const openSurface = useCallback(
    (focusFirst: boolean) => {
      setStatusError(null);
      setAutoFocus(focusFirst);
      setOpen(native ? "menu" : "status");
    },
    [native],
  );

  // Escape closes and hands focus back to the indicator. A click elsewhere, or focus moving
  // anywhere outside the surface and the indicator, closes without moving focus: GNOME's
  // menus do both.
  useEffect(() => {
    if (!open) return;
    const outside = (t: EventTarget | null) => {
      const n = t as Node | null;
      if (!n || surface.current?.contains(n) || indicator.current?.contains(n)) return false;
      return !(n as Element).closest?.(".tray-legend, .g-toast, .g-tooltip");
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") close(true);
    };
    const onDown = (e: PointerEvent) => outside(e.target) && close(false);
    const onFocus = (e: FocusEvent) => {
      const n = e.target as Element | null;
      if (n && !surface.current?.contains(n) && !indicator.current?.contains(n)) close(false);
    };
    window.addEventListener("keydown", onKey);
    window.addEventListener("pointerdown", onDown);
    window.addEventListener("focusin", onFocus);
    return () => {
      window.removeEventListener("keydown", onKey);
      window.removeEventListener("pointerdown", onDown);
      window.removeEventListener("focusin", onFocus);
    };
  }, [open, close]);

  const scenario = design?.scenario.name ?? null;
  const openHref = scenarioHref(scenario, state.line && status?.conversation ? `/lines/${status.conversation}` : "/");
  const label = `Ghost: ${state.title}${pending ? ", waiting for your okay" : ""}`;
  const dot = pending ? null : state.presence === "passive" ? null : state.presence;
  const current = stateName(state, !!pending, live.connection, live.setupNeeded);
  const hrefWith = (over: Record<string, string | null>) => {
    const q = new URLSearchParams(window.location.search);
    for (const [k, v] of Object.entries(over)) {
      if (v == null) q.delete(k);
      else q.set(k, v);
    }
    q.delete("menu");
    return `${window.location.pathname}?${q.toString()}#/tray`;
  };

  const surfaceEl =
    open === "menu" ? (
      <TrayNativeMenu
        ref={(el) => {
          surface.current = el;
        }}
        id={SURFACE_ID}
        state={state}
        autoFocus={autoFocus}
        onMutedChange={changeMuted}
        onClose={close}
        onShowStatus={(err) => {
          setStatusError(err ?? null);
          setAutoFocus(usingKeyboard());
          setOpen("status");
        }}
        openHref={openHref}
        onQuit={quitApp}
      />
    ) : open === "status" ? (
      <TrayPopover
        ref={(el) => {
          surface.current = el;
        }}
        key={statusError ?? "status"}
        id={SURFACE_ID}
        state={state}
        shortcut={data?.shortcut ?? null}
        autoFocus={autoFocus}
        initialError={statusError}
        onMutedChange={changeMuted}
        openHref={openHref}
        onQuit={quitApp}
      />
    ) : null;

  return (
    <div ref={stage} className="tray-stage">
      <div className="tray-topbar" data-theme="dark">
        <div className="tray-topbar__start" aria-hidden>
          <span className="tray-ws is-current" />
          <span className="tray-ws" />
        </div>
        <div className="tray-topbar__clock t-num" aria-hidden>
          {data?.clock ?? "Sat Oct 18  3:42 PM"}
        </div>
        <div className="tray-topbar__end">
          {!quit && (
            <button
              ref={indicator}
              type="button"
              className={cx("tray-indicator", open && "is-open", state.presence === "offline" && "is-offline")}
              aria-label={label}
              title={native ? label : undefined}
              aria-expanded={!!open}
              aria-controls={open ? SURFACE_ID : undefined}
              aria-haspopup={native ? "menu" : "dialog"}
              onClick={(e) => {
                if (open) close(false);
                // Enter and Space on a native menu open it with the first item focused.
                else openSurface(native && e.detail === 0);
              }}
              onKeyDown={(e) => {
                if (e.key !== "ArrowDown") return;
                e.preventDefault();
                if (open) actionables(surface.current)[0]?.focus();
                else openSurface(true);
              }}
            >
              <GhostGlyph size={16} />
              {/* Both signals when both hold: a waiting change has a clock on it, and the
                  HUD sends the operator here, so muted never hides it. */}
              {state.muted && <MicOff className="tray-indicator__mute" aria-hidden />}
              {pending ? (
                <span className="tray-indicator__dot is-waiting" aria-hidden />
              ) : dot && !state.muted ? (
                <StatusDot phase={dot} size="sm" still className="tray-indicator__status" />
              ) : null}
            </button>
          )}
          <span className="tray-sys" aria-hidden>
            <Wifi />
            <Volume2 />
            <BatteryFull />
          </span>
        </div>
      </div>

      {!quit && surfaceEl}

      {quit && (
        <div className="tray-quit" role="status">
          <p className="tray-quit__title">The app is closed</p>
          <p className="tray-quit__text">
            {live.online && !muted ? (
              <>
                Ghost keeps running and still answers to <span className="tray-nowrap">{quoted("Hey Ghost")}</span>.
              </>
            ) : (
              "Ghost keeps running on its own."
            )}{" "}
            Open the app again to bring the icon back.
          </p>
          <Button ref={reopen} size="sm" onClick={reopenApp}>
            Open the app
          </Button>
        </div>
      )}

      {/* Stage scaffolding, after the surface so Tab goes from the indicator into it. */}
      <nav className="tray-legend" aria-label="Tray states">
        <h1 className="tray-legend__title">
          Tray <Badge variant="outline" size="sm">Prototype</Badge>
        </h1>
        <p className="tray-legend__note">Ghost in the Ubuntu top bar at real size. Mock data only.</p>

        <p className="tray-legend__label" id="tray-legend-open">
          Opens from the icon
        </p>
        <ul className="tray-legend__list" aria-labelledby="tray-legend-open">
          {[
            { label: "Native menu", on: native, href: hrefWith({ native: "1" }) },
            { label: "Status window", on: !native, href: hrefWith({ native: null }) },
          ].map((s) => (
            <li key={s.label}>
              <a className={cx("tray-legend__link", s.on && "is-current")} href={s.href} aria-current={s.on ? "true" : undefined}>
                {s.label}
              </a>
            </li>
          ))}
        </ul>

        <p className="tray-legend__label" id="tray-legend-state">
          State
        </p>
        <ul className="tray-legend__list" aria-labelledby="tray-legend-state">
          {STATES.map((s) => {
            const on = current === s.label;
            const href = hrefWith({ mock: s.mock, muted: s.muted ? "1" : null });
            return (
              <li key={s.label}>
                <a className={cx("tray-legend__link", on && "is-current")} href={href} aria-current={on ? "true" : undefined}>
                  {s.label}
                </a>
              </li>
            );
          })}
        </ul>

        <div className="tray-legend__notes">
          <p>
            <strong>On Ubuntu</strong> a click on the icon opens GNOME’s own menu (dbusmenu): text, checkboxes and separators,
            nothing else. <strong>Show status…</strong> opens the status window, an undecorated window of Ghost’s own.
          </p>
          <p>
            <strong>X11:</strong> the status window opens under the icon at the top right; the HUD sits 48px above the bottom edge,
            centered.
          </p>
          <p>
            <strong>Wayland:</strong> GNOME places app windows itself. The status window opens where Mutter puts it, centered by
            default. The HUD needs a small GNOME Shell extension to sit at the bottom center; without one it stays off and the
            icon’s dot carries the state.
          </p>
        </div>
      </nav>

      {!quit && (
        <TrayHud
          state={state}
          onSleepStart={() => stageFocus.request(() => indicator.current)}
        />
      )}
    </div>
  );
}
