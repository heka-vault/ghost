/**
 * Owner: design. Two concepts that push past the direction being built
 * (docs/design/concepts-2026-11.md), for the owner to open and react to. Review only: nothing
 * here is a screen, and nothing reads or changes the daemon.
 *
 *   instrument   Ghost as a machine you play: one field, Wisp as its core, the work as light.
 *   signal       Only language and time: one living line of type, telemetry in the margin.
 *
 * `#/concepts/<name>` with `?mock=busy`. URL options: `at=<seconds>` starts the script there
 * (screenshots), `motion=reduced` forces the reduced-motion variant, `theme=light|dark`.
 * Keys: Space pauses, R restarts the sequence, End returns to now.
 */
import { useEffect, useMemo, useRef, useState } from "react";

import { Link } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { createEngine } from "./engine";
import Instrument from "./instrument";
import { FrameContext, type Frame, urlOption, useFrame, useKey } from "./shared";
import Signal from "./signal";
import "./concepts.css";

type Name = "instrument" | "signal";
const NAMES: { name: Name; label: string }[] = [
  { name: "instrument", label: "The instrument" },
  { name: "signal", label: "The signal" },
];

export default function Concepts({ params }: RouteProps) {
  const name: Name = params.name === "signal" ? "signal" : "instrument";
  const reduced = useMemo(() => {
    const o = urlOption("motion");
    if (o) return o === "reduced";
    return window.matchMedia?.("(prefers-reduced-motion: reduce)").matches ?? false;
  }, []);
  const theme = useMemo<"dark" | "light">(() => {
    const o = urlOption("theme");
    if (o === "light" || o === "dark") return o;
    return window.matchMedia?.("(prefers-color-scheme: light)").matches ? "light" : "dark";
  }, []);
  const handle = useMemo(() => createEngine(reduced), [reduced]);
  const subs = useRef(new Set<(now: number) => void>());

  useEffect(() => {
    const at = Number(urlOption("at"));
    if (at) handle.jump(at);
    let raf = 0;
    const loop = (now: number) => {
      handle.tick(now);
      subs.current.forEach((f) => f(now));
      raf = requestAnimationFrame(loop);
    };
    raf = requestAnimationFrame(loop);
    return () => cancelAnimationFrame(raf);
  }, [handle]);

  // Restart the sequence whenever the concept changes, so each opens on its idle state.
  const shownName = useRef(name);
  useEffect(() => {
    if (shownName.current === name) return;
    shownName.current = name;
    handle.restart();
  }, [name, handle]);

  useEffect(() => {
    const key = (ev: KeyboardEvent) => {
      if (ev.target instanceof HTMLElement && ev.target.closest("input, textarea")) return;
      if (ev.key === " " && !(ev.target instanceof HTMLButtonElement)) {
        ev.preventDefault();
        handle.togglePause();
      } else if (ev.key === "r" || ev.key === "R") handle.restart();
      else if (ev.key === "End" || ev.key === "Escape") handle.toLive();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
  }, [handle]);

  const frame = useMemo<Frame>(
    () => ({
      handle,
      reduced,
      theme,
      onFrame(f) {
        subs.current.add(f);
        return () => subs.current.delete(f);
      },
    }),
    [handle, reduced, theme],
  );

  // Concepts set their own ground. The instrument is dark only: light on white loses the glow
  // it is made of (docs/design/concepts-2026-11.md). The signal follows the theme.
  return (
    <FrameContext.Provider value={frame}>
      <div className={`cx cx--${name}`} data-theme={name === "instrument" ? "dark" : theme} data-reduced={reduced || undefined}>
        {name === "instrument" ? <Instrument /> : <Signal />}
        <Switcher name={name} />
      </div>
    </FrameContext.Provider>
  );
}

function Switcher({ name }: { name: Name }) {
  const frame = useFrame();
  const paused = useKey((h) => h.e.paused);
  const [help, setHelp] = useState(false);
  const qs = window.location.search;
  return (
    <nav className="cx-switch" aria-label="Concepts">
      {NAMES.map((n) => (
        <Link key={n.name} to={`/concepts/${n.name}`} className={n.name === name ? "cx-switch__item is-on" : "cx-switch__item"} aria-current={n.name === name ? "page" : undefined}>
          {n.label}
        </Link>
      ))}
      <span className="cx-switch__sep" aria-hidden />
      <button type="button" className="cx-switch__item" onClick={() => frame.handle.togglePause()} aria-pressed={paused}>
        {paused ? "Play" : "Pause"}
      </button>
      <button type="button" className="cx-switch__item" onClick={() => frame.handle.restart()}>
        Replay sequence
      </button>
      <button type="button" className="cx-switch__item" onClick={() => setHelp((v) => !v)} aria-expanded={help}>
        Keys
      </button>
      {help && (
        <div className="cx-switch__help" role="note">
          {name === "instrument" ? "Drag across the field or scroll to pull time; ← → step 5 min (Shift: 1 h). " : "Scroll or drag the river to move through the day; ↑ ↓ one line (Shift: 1 h). "}
          Y / N answer a decision, 1 to 3 a question. End returns to now. Space pauses, R replays.
          {qs.includes("motion=reduced") ? "" : " Add &motion=reduced for the reduced-motion variant."}
        </div>
      )}
    </nav>
  );
}
