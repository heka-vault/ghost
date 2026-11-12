/**
 * Ghost, floating: the summon sequence over the desktop. SPIKE (docs/design/floating-2026-11.md).
 *
 * Three layers, back to front: the field (field.ts, one WebGL2 pass at half resolution: dim,
 * haze, glow, voice energy, reach beams, scanned light), Wisp (the brand's own mascot engine,
 * SVG), and the words (DOM type, WAAPI for the zoom). No motion library: the frame loop is
 * ours, runs only while Ghost is on screen, and stops dead at idle, so idle costs nothing.
 *
 * Driven by `window.__floating.command("summon" | "demo" | "dismiss" | "state <phase>" |
 * "mic off" | "mic on" | "yes" | "no")`, which the window's socket delivers, and by keys when
 * the window has focus: Space summons or plays the demo, Escape dismisses, Y and N answer.
 */
import { splitSaid, CHARS_PER_VOICED_SEC } from "@ghost/ui";
import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import { Mascot } from "../../ui/Mascot";
import { speechEnvelope } from "../../ui/VoiceWave";
import { createField, type Field, type FieldInputs } from "./field";
import { AFTER_NO, AFTER_YES, LABEL, OKAY, type Phase, REACH, REPLY, TIMING, TINT } from "./script";
import { hitRects, report, setRegion, setVisible } from "./shell";

const params = new URLSearchParams(window.location.search);
const reducedQuery = typeof matchMedia !== "undefined" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
const reduced = () => params.get("still") === "1" || (reducedQuery?.matches ?? false);
const RES = Number(params.get("res") ?? 0.5);
/** Measurement switches: `?field=0` stops drawing the field, `?wisp=0` leaves Wisp out. */
const DRAW_FIELD = params.get("field") !== "0";
const DRAW_WISP = params.get("wisp") !== "0";

const MOOD: Record<Phase, string | null> = {
  idle: null, wake: "starting", listening: "listening", thinking: "thinking", speaking: "speaking", okay: "listening", dismiss: null,
};

/** Approach `to` from `from` at `rate` per second, frame-rate independent. */
const ease = (from: number, to: number, rate: number, dt: number) => from + (to - from) * (1 - Math.exp(-rate * dt));

interface Live {
  phase: Phase;
  since: number;
  /** The reply being said has been said in full (the demo moves on once). */
  done: boolean;
  demo: boolean;
  text: string;
  voiced: number;
  dim: number; glow: number; ring: number; okay: number; voice: number; level: number;
  tint: [number, number, number];
  reach: number[];
  scan: number;
}

export function Floating() {
  const [phase, setPhaseState] = useState<Phase>("idle");
  const [mic, setMic] = useState(true);
  const [reply, setReply] = useState(REPLY);
  const [trigger, setTrigger] = useState(0);
  const root = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const wisp = useRef<HTMLDivElement>(null);
  const saidEl = useRef<HTMLSpanElement>(null);
  const restEl = useRef<HTMLSpanElement>(null);
  const nodeEls = useRef<(HTMLDivElement | null)[]>([]);
  const field = useRef<Field | null>(null);
  const raf = useRef(0);
  const timers = useRef<number[]>([]);
  const live = useRef<Live>({
    phase: "idle", since: 0, done: false, demo: false, text: REPLY, voiced: 0,
    dim: 0, glow: 0, ring: 0, okay: 0, voice: 0, level: 0, tint: TINT.idle, reach: REACH.map(() => 0), scan: -1,
  });
  const stats = useRef({ frames: 0, js: 0, gaps: [] as number[], last: 0, from: 0 });

  // ------------------------------------------------------------------ geometry
  const geometry = useCallback(() => {
    const w = window.innerWidth;
    const h = window.innerHeight;
    const core: [number, number] = [w / 2, h * 0.37];
    const m = Math.min(w, h);
    const nodes = REACH.map((_, i) => {
      const a = Math.PI * (1.1 + (i / (REACH.length - 1)) * 0.8); // an arc over the head, outside the ring
      return { x: core[0] + Math.cos(a) * m * 0.46, y: core[1] + 0.06 * m + Math.sin(a) * m * 0.3 };
    });
    return { w, h, core, nodes };
  }, []);

  // ------------------------------------------------------------------ the frame
  const frame = useCallback((now: number) => {
    const L = live.current;
    const f = field.current;
    const st = stats.current;
    const t0 = performance.now();
    const dt = st.last ? Math.min(0.1, (now - st.last) / 1000) : 1 / 60;
    if (st.last) st.gaps.push(now - st.last);
    st.last = now;
    const t = now / 1000;
    const inPhase = t - L.since;
    const g = geometry();

    const target = {
      dim: L.phase === "idle" || L.phase === "dismiss" ? 0 : L.phase === "speaking" ? 0.46 : 0.58,
      glow: L.phase === "idle" || L.phase === "dismiss" ? 0 : L.phase === "wake" ? 1.3 : 1,
      ring: L.phase === "listening" ? 1 : L.phase === "okay" ? 0.35 : 0,
      okay: L.phase === "okay" ? 1 : 0,
    };
    const fast = L.phase === "dismiss" ? 9 : 5;
    L.dim = ease(L.dim, target.dim, fast, dt);
    L.glow = ease(L.glow, target.glow, fast, dt);
    L.ring = ease(L.ring, target.ring, 6, dt);
    L.okay = ease(L.okay, target.okay, 5, dt);
    const tint = TINT[L.phase];
    L.tint = [ease(L.tint[0], tint[0], 4, dt), ease(L.tint[1], tint[1], 4, dt), ease(L.tint[2], tint[2], 4, dt)];

    // The operator's voice, mock: energy only, never words.
    const you = L.phase === "listening" && inPhase > 0.3 ? speechEnvelope(inPhase * 1.1 + 3.7) : 0;
    L.voice = ease(L.voice, you, 18, dt);
    // Ghost's own voice level, mock; it also clocks the said words.
    const ghost = L.phase === "speaking" ? speechEnvelope(inPhase + 0.4) : 0;
    L.level = ease(L.level, ghost, 22, dt);
    if (L.phase === "speaking" && ghost > 0.05) L.voiced += dt;

    // Reach: each call lights while it runs and stays dim once it has answered.
    L.reach = REACH.map((r, i) => {
      if (L.phase !== "thinking") return ease(L.reach[i], 0, 6, dt);
      const on = inPhase >= r.at && inPhase < r.at + r.takes ? 1 : inPhase >= r.at + r.takes ? 0.28 : 0;
      return ease(L.reach[i], on, 10, dt);
    });
    nodeEls.current.forEach((el, i) => {
      if (!el) return;
      el.style.opacity = String(Math.min(1, L.reach[i] * 1.6));
      el.dataset.on = L.reach[i] > 0.6 ? "1" : "0";
    });

    // The scanned light: once down the screen on the wake.
    L.scan = L.phase === "wake" ? Math.min(1.05, inPhase / (TIMING.wake * 0.9)) : -1;

    if (L.phase === "speaking" && saidEl.current && restEl.current) {
      const chars = Math.min(L.text.length, Math.floor(L.voiced * CHARS_PER_VOICED_SEC));
      const { said, rest } = splitSaid(L.text, chars);
      if (saidEl.current.textContent !== said) {
        saidEl.current.textContent = said;
        restEl.current.textContent = rest;
      }
      if (chars >= L.text.length && inPhase > 0.6 && L.demo && !L.done) {
        L.done = true;
        advance();
      }
    }

    if (wisp.current) {
      const s = 1 + 0.035 * L.level + 0.02 * L.voice;
      wisp.current.style.setProperty("--flt-pulse", s.toFixed(4));
    }

    // The arrival's shockwave, as the zoom lands; on dismissal the light falls to the ember.
    const shock = L.phase === "wake" ? (inPhase - TIMING.wake * 0.62) / 0.7 : L.phase === "listening" && inPhase < 0.5 ? (inPhase + TIMING.wake * 0.38) / 0.7 : -1;
    const fall = L.phase === "dismiss" ? Math.min(1, inPhase / TIMING.dismiss) : 0;
    const core: [number, number] = [g.core[0], g.core[1] + (g.h - g.core[1]) * fall * fall];
    const inputs: FieldInputs = {
      shock: shock >= 0 && shock <= 1 ? shock : -1,
      time: t, dim: L.dim, scan: L.scan, core, glow: L.glow, tint: L.tint,
      voice: L.voice, ring: L.ring, level: L.level, okay: L.okay,
      nodes: g.nodes.map((n, i) => ({ x: n.x, y: n.y, on: L.reach[i] })),
    };
    if (DRAW_FIELD) f?.draw(inputs);

    st.frames++;
    st.js += performance.now() - t0;
    if (now - st.from > 2000) flushStats(now);

    const settled = L.phase === "dismiss" && L.dim < 0.004 && L.glow < 0.004;
    if (settled) {
      toIdle();
      return;
    }
    if (reduced()) return; // one frame per change under reduced motion
    raf.current = requestAnimationFrame(frame);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [geometry]);

  const flushStats = (now: number) => {
    const st = stats.current;
    if (st.frames > 2) {
      const gaps = [...st.gaps].sort((a, b) => a - b);
      const p95 = gaps[Math.floor(gaps.length * 0.95)] ?? 0;
      const mean = gaps.reduce((a, b) => a + b, 0) / Math.max(1, gaps.length);
      const long = gaps.filter((x) => x > 25).length;
      report(`frames phase=${live.current.phase} fps=${(1000 / mean).toFixed(1)} p95_ms=${p95.toFixed(1)} long=${long}/${gaps.length} js_ms=${(st.js / st.frames).toFixed(2)}`);
    }
    st.frames = 0;
    st.js = 0;
    st.gaps = [];
    st.from = now;
  };

  const start = () => {
    cancelAnimationFrame(raf.current);
    stats.current.last = 0;
    stats.current.from = performance.now();
    if (canvas.current && field.current?.ok) canvas.current.style.visibility = "visible";
    raf.current = requestAnimationFrame(frame);
  };

  const toIdle = () => {
    cancelAnimationFrame(raf.current);
    flushStats(performance.now());
    field.current?.clear();
    if (canvas.current) canvas.current.style.visibility = "hidden";
    live.current.phase = "idle";
    setPhaseState("idle");
    setVisible(false);
    report("idle: loop stopped, canvas hidden");
  };

  // ------------------------------------------------------------------ phases
  const clearTimers = () => {
    timers.current.forEach((id) => clearTimeout(id));
    timers.current = [];
  };
  const after = (secs: number, fn: () => void) => timers.current.push(window.setTimeout(fn, secs * 1000));

  const setPhase = useCallback((p: Phase) => {
    const L = live.current;
    const was = L.phase;
    L.phase = p;
    L.done = false;
    L.since = performance.now() / 1000;
    if (p === "speaking") {
      L.voiced = 0;
      if (saidEl.current && restEl.current) {
        saidEl.current.textContent = "";
        restEl.current.textContent = L.text;
      }
    }
    setPhaseState(p);
    if (p === "wake") setTrigger((n) => n + 1);
    if (was === "idle" && p !== "idle") {
      setVisible(true);
      start();
    } else if (reduced()) {
      raf.current = requestAnimationFrame(frame);
    }
    report(`phase ${p}`);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [frame]);

  /** The demo's next step from the current phase. */
  const advance = () => {
    const L = live.current;
    clearTimers();
    switch (L.phase) {
      case "wake": return setPhase("listening");
      case "listening": return setPhase("thinking");
      case "thinking":
        L.text = REPLY;
        setReply(REPLY);
        return setPhase("speaking");
      case "speaking":
        if (L.text === REPLY) {
          setPhase("okay");
          if (L.demo) after(TIMING.okayAuto, () => answer(true));
          return;
        }
        return after(0.9, () => setPhase("dismiss"));
      default: return;
    }
  };

  const answer = (yes: boolean) => {
    const L = live.current;
    if (L.phase !== "okay") return;
    clearTimers();
    L.text = yes ? AFTER_YES : AFTER_NO;
    setReply(L.text);
    setPhase("speaking");
  };

  const summon = (demo: boolean) => {
    const L = live.current;
    clearTimers();
    L.demo = demo;
    L.text = REPLY;
    setReply(REPLY);
    setPhase("wake");
    after(TIMING.wake, () => {
      setPhase("listening");
      if (demo) {
        after(TIMING.listening, () => {
          setPhase("thinking");
          after(TIMING.thinking, advance);
        });
      }
    });
  };

  const dismiss = () => {
    clearTimers();
    if (live.current.phase !== "idle") setPhase("dismiss");
  };

  const command = (raw: string) => {
    const [cmd, arg] = raw.trim().split(/\s+/);
    if (cmd === "summon") summon(false);
    else if (cmd === "demo") summon(true);
    else if (cmd === "dismiss") dismiss();
    else if (cmd === "yes" || cmd === "no") answer(cmd === "yes");
    else if (cmd === "mic") setMic(arg !== "off");
    else if (cmd === "state" && arg && arg in TINT) {
      clearTimers();
      live.current.demo = false;
      if (arg === "speaking") live.current.text = REPLY;
      if (arg === "thinking") live.current.since = performance.now() / 1000;
      setPhase(arg as Phase);
    } else report(`unknown command: ${raw}`);
  };

  // ------------------------------------------------------------------ setup
  useEffect(() => {
    if (!canvas.current) return;
    try {
      field.current = createField(canvas.current, RES);
    } catch (e) {
      report(`field failed: ${String(e)}`);
    }
    report(`ready: webgl2=${field.current?.ok ?? false} renderer="${field.current?.renderer ?? "none"}" dpr=${window.devicePixelRatio} viewport=${window.innerWidth}x${window.innerHeight} res=${RES} reduced=${reduced()} ua="${navigator.userAgent}"`);
    canvas.current.style.visibility = "hidden";
    const onResize = () => {
      field.current?.resize();
      report(`resize ${window.innerWidth}x${window.innerHeight} dpr=${window.devicePixelRatio}`);
    };
    window.addEventListener("resize", onResize);
    const onKey = (e: KeyboardEvent) => {
      if (e.key === " ") command(live.current.phase === "idle" ? "demo" : "dismiss");
      else if (e.key === "Escape") command("dismiss");
      else if (e.key === "y") command("yes");
      else if (e.key === "n") command("no");
      else if (e.key === "s") command("summon");
    };
    window.addEventListener("keydown", onKey);
    (window as unknown as { __floating: { command: (c: string) => void } }).__floating = { command };
    const auto = params.get("auto");
    if (auto) window.setTimeout(() => command(auto), 600);
    return () => {
      window.removeEventListener("resize", onResize);
      window.removeEventListener("keydown", onKey);
      cancelAnimationFrame(raf.current);
      clearTimers();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // The zoom from depth, and back. WAAPI: the compositor runs it, no library.
  useLayoutEffect(() => {
    const el = wisp.current;
    if (!el) return;
    const quick = reduced();
    if (phase === "wake") {
      el.animate(
        [
          { transform: "perspective(900px) translate3d(0, 40px, -2600px)", filter: "blur(18px) brightness(3)", opacity: 0 },
          { transform: "perspective(900px) translate3d(0, -6px, 40px)", filter: "blur(0px) brightness(1.6)", opacity: 1, offset: 0.78 },
          { transform: "perspective(900px) translate3d(0, 0, 0)", filter: "blur(0px) brightness(1)", opacity: 1 },
        ],
        { duration: quick ? 0 : TIMING.wake * 1000, easing: "cubic-bezier(.16,.84,.24,1)", fill: "both" },
      );
    } else if (phase === "dismiss") {
      el.animate(
        [
          { transform: "perspective(900px) translate3d(0, 0, 0)", filter: "blur(0px)", opacity: 1 },
          { transform: "perspective(900px) translate3d(0, 30vh, -1800px)", filter: "blur(10px)", opacity: 0 },
        ],
        { duration: quick ? 0 : TIMING.dismiss * 1000, easing: "cubic-bezier(.5,0,.75,0)", fill: "both" },
      );
    }
  }, [phase]);

  // The input region follows what is drawn: nothing at idle, Wisp and any buttons otherwise.
  useEffect(() => {
    if (!root.current) return;
    const send = () => setRegion(phase === "idle" ? [] : hitRects(root.current!));
    send();
    const id = window.setTimeout(send, 400); // after transitions settle
    return () => clearTimeout(id);
  }, [phase, reply]);

  const g = geometry();
  const shown = phase !== "idle";
  return (
    <div ref={root} className={params.get("textglow") === "0" ? "flt-root flt-noglow" : "flt-root"} data-phase={phase} data-mic={mic ? "on" : "off"}>
      <canvas ref={canvas} className="flt-field" aria-hidden />

      {/* Idle: one still ember at the bottom edge says Ghost is hearing for its name; red and
          struck when the microphone is off (ADR 0057). Static pixels, no loop. */}
      <div className="flt-ember" aria-hidden={shown} role="status" aria-label={mic ? "Ghost is listening for its name" : "Ghost is not listening"}>
        {mic ? <span className="flt-ember__line" /> : <MicOff />}
      </div>

      {shown && <div className="flt-stage" style={{ left: g.core[0], top: g.core[1] }}>
        <div ref={wisp} className="flt-wisp" data-hit onClick={() => command("dismiss")} title="Dismiss">
          {DRAW_WISP && <Mascot size={224} mood={phase === "dismiss" ? null : MOOD[phase]} trigger={trigger} idle />}
        </div>
        <p className="flt-label" aria-live="polite">
          <span className="flt-label__dot" />
          {LABEL[phase]}
          {mic ? null : <span className="flt-label__mic"> · mic off</span>}
        </p>
      </div>}

      {REACH.map((r, i) => (
        <div
          key={r.id}
          ref={(el) => { nodeEls.current[i] = el; }}
          className="flt-node"
          data-kind={r.kind}
          style={{ left: g.nodes[i].x, top: g.nodes[i].y }}
          hidden={!shown}
        >
          <span className="flt-node__name">{r.name}</span>
          <span className="flt-node__detail">{r.kind === "agent" ? `agent · ${r.detail}` : r.detail}</span>
        </div>
      ))}

      <div className="flt-words" hidden={!(phase === "speaking" || phase === "okay")} style={{ top: g.h * 0.37 + Math.min(g.w, g.h) * 0.27 }}>
        {phase === "okay" ? (
          <div className="flt-okay" role="alertdialog" aria-label={`${OKAY.asker} asks: ${OKAY.summary}?`}>
            <p className="flt-okay__over">{OKAY.asker} asks · <span className="flt-mono">{OKAY.operation}</span></p>
            <p className="flt-okay__q">{OKAY.summary}?</p>
            <div className="flt-okay__acts">
              <button type="button" data-hit className="flt-btn flt-btn--yes" onClick={() => answer(true)}>Yes, close it</button>
              <button type="button" data-hit className="flt-btn" onClick={() => answer(false)}>Leave it</button>
              <span className="flt-okay__or">or say “yes”</span>
            </div>
          </div>
        ) : (
          <p className="flt-reply" aria-label={reply}>
            <span ref={saidEl} className="flt-reply__said" aria-hidden />
            <span ref={restEl} className="flt-reply__rest" aria-hidden>{reply}</span>
          </p>
        )}
      </div>
    </div>
  );
}

function MicOff() {
  return (
    <svg className="flt-ember__mic" viewBox="0 0 24 24" width="16" height="16" aria-hidden>
      <path d="M9 9v3a3 3 0 0 0 5.1 2.1M15 9.3V5a3 3 0 0 0-5.9-.7M19 10v2a7 7 0 0 1-.6 2.8M5 10v2a7 7 0 0 0 11.6 5.3M12 19v3M2 2l20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}
