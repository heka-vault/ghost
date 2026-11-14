/**
 * Owner: design. Concept: the instrument. Ghost as a machine you play. One dark field; Wisp is
 * its core and visibly works: your voice streams into it from where you sit, its own voice
 * leaves it as rings, every call goes out to an app and comes back as light sized by what it
 * brought. Agents are bodies in orbit, and their trails are where they were when they did
 * their work. What needs you rises out of the field to the front and waits. Time is the
 * gesture: drag across the field and the whole field replays the day.
 *
 * Hand-written canvas 2D, no dependency. Type and buttons are DOM, over the canvas, so they
 * stay crisp and reachable by keyboard and screen reader.
 */
import { wispGlyph } from "@ghost/ui";
import { useCallback, useEffect, useMemo, useRef, useState } from "react";

import { agentOf, AGENTS, type AgentId, APPS, type Ask, type Call, clock, type Day, DAY_START, H, levelAt, M, size, type State } from "./day";
import { alpha, fitCanvas, resolveColors, useFrame, useKey } from "./shared";

const COLORS = {
  ground: "#06060a",
  text1: "var(--text-1)",
  text2: "var(--text-2)",
  text3: "var(--text-3)",
  warn: "var(--warn)",
  passive: "var(--phase-passive)",
  listening: "var(--phase-listening)",
  thinking: "var(--phase-thinking)",
  speaking: "var(--phase-speaking)",
  h0: "var(--hue-0-fg)",
  h1: "var(--hue-1-fg)",
  h2: "var(--hue-2-fg)",
  h3: "var(--hue-3-fg)",
  h4: "var(--hue-4-fg)",
  h5: "var(--hue-5-fg)",
};
type Palette = Record<keyof typeof COLORS, string>;

interface Layout {
  w: number;
  h: number;
  cx: number;
  cy: number;
  /** Radius of the app ring. */
  S: number;
  core: number;
  small: boolean;
  /** Foreground: where what needs you waits. */
  slotY: number;
  captionY: number;
  axisY: number;
}

function layout(w: number, h: number): Layout {
  const small = w < 760 || h < 560;
  const axisY = h - (small ? 22 : 30);
  const cy = h * (small ? 0.38 : 0.44);
  const S = Math.max(90, Math.min(w * 0.5 - (small ? 44 : 110), cy - (small ? 30 : 48)));
  return { w, h, cx: w / 2, cy, S, core: Math.max(16, S * 0.12), small, slotY: axisY - (small ? 96 : 132), captionY: cy + S * 0.74, axisY };
}

const TAU = Math.PI * 2;
const hueOf = (p: Palette, id: AgentId) => p[`h${agentOf(id).hue}` as keyof Palette];

function agentPos(L: Layout, id: AgentId, t: number) {
  const a = agentOf(id);
  if (!a.orbit) return { x: L.cx, y: L.cy, angle: 0 };
  const angle = a.phase0 + a.speed * TAU * (t / H);
  const r = a.orbit * L.S * 1.25;
  return { x: L.cx + Math.cos(angle) * r, y: L.cy + Math.sin(angle) * r * 0.86, angle };
}

function appPos(L: Layout, id: string) {
  const app = APPS.find((a) => a.id === id)!;
  return { x: L.cx + Math.cos(app.angle) * L.S, y: L.cy + Math.sin(app.angle) * L.S * 0.86 };
}

/** The slot an open item waits in, at the front. */
function slotX(L: Layout, i: number, n: number) {
  const gap = L.small ? Math.min(L.w - 32, 300) : Math.min(420, (L.w - 120) / Math.max(1, n));
  return L.cx + (i - (n - 1) / 2) * gap;
}

/** A point on the quadratic curve a call travels, bowed away from the core. */
function bez(ax: number, ay: number, bx: number, by: number, L: Layout, k: number) {
  // From the core: nearly straight. From an orbit: around the outside, never through the core.
  const ra = Math.hypot(ax - L.cx, ay - L.cy);
  let qx = (ax + bx) / 2;
  let qy = (ay + by) / 2;
  if (ra > L.core * 2) {
    const ua = Math.atan2(ay - L.cy, ax - L.cx);
    const ub = Math.atan2(by - L.cy, bx - L.cx);
    let d = ub - ua;
    while (d > Math.PI) d -= TAU;
    while (d < -Math.PI) d += TAU;
    const m = ua + d / 2;
    const rr = Math.max(ra, Math.hypot(bx - L.cx, by - L.cy)) * (0.95 + Math.abs(d) * 0.12);
    qx = L.cx + Math.cos(m) * rr;
    qy = L.cy + Math.sin(m) * rr * 0.86;
  }
  const u = 1 - k;
  return { x: u * u * ax + 2 * u * k * qx + k * k * bx, y: u * u * ay + 2 * u * k * qy + k * k * by, qx, qy };
}

/** The last frames' draw times, for the performance note (read from the console: __ghostDrawMs). */
const drawMs: number[] = [];
(window as unknown as { __ghostDrawMs: number[] }).__ghostDrawMs = drawMs;

export default function Instrument() {
  const { handle, onFrame, reduced } = useFrame();
  const canvas = useRef<HTMLCanvasElement>(null);
  const root = useRef<HTMLDivElement>(null);
  const [box, setBox] = useState({ w: 1440, h: 900 });
  const L = useMemo(() => layout(box.w, box.h), [box]);
  const glyph = useMemo(() => new Path2D(wispGlyph(false)), []);
  const palette = useRef<Palette | null>(null);
  const perAgent = useMemo(() => {
    const m = new Map<AgentId, Call[]>();
    for (const a of AGENTS) m.set(a.id, handle.e.day.calls.filter((c) => c.agent === a.id));
    return m;
  }, [handle]);

  useEffect(() => {
    const el = root.current!;
    palette.current = resolveColors(el, COLORS);
    const ro = new ResizeObserver(() => setBox({ w: el.clientWidth, h: el.clientHeight }));
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  // ---------------------------------------------------------------- drawing
  useEffect(() => {
    const c = canvas.current!;
    const ctx = c.getContext("2d")!;
    return onFrame((now) => {
      const p = palette.current;
      if (!p) return;
      const { dpr } = fitCanvas(c);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      const e = handle.e;
      const t = e.view;
      const s = handle.state();
      const t0 = performance.now();
      draw(ctx, L, p, s, t, e.live, e.mode !== "live", reduced, glyph, perAgent, e.day, now / 1000);
      drawMs.push(performance.now() - t0);
      if (drawMs.length > 240) drawMs.shift();
    });
  }, [onFrame, handle, L, reduced, glyph, perAgent]);

  // ---------------------------------------------------------------- the gesture: pull time
  const drag = useRef<{ x: number; moved: boolean; axis: boolean } | null>(null);
  const secondsPerPx = L.small ? 30 : 14;
  const axisToT = useCallback((x: number) => DAY_START + ((x - 16) / (L.w - 32)) * (handle.e.live - DAY_START), [L, handle]);
  const letGo = () => {
    if (handle.e.live - handle.e.view < 90) handle.toLive();
  };
  const onPointerDown = (ev: React.PointerEvent) => {
    const r = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    const y = ev.clientY - r.top;
    const axis = y > L.axisY - 16;
    drag.current = { x: ev.clientX, moved: false, axis };
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
    if (axis) handle.seek(axisToT(ev.clientX - r.left));
  };
  const onPointerMove = (ev: React.PointerEvent) => {
    const d = drag.current;
    if (!d) return;
    const r = (ev.currentTarget as HTMLElement).getBoundingClientRect();
    if (d.axis) handle.seek(axisToT(ev.clientX - r.left));
    else handle.pull(-(ev.clientX - d.x) * secondsPerPx);
    d.x = ev.clientX;
    d.moved = true;
  };
  const onPointerUp = () => {
    drag.current = null;
    letGo();
  };
  const onWheel = (ev: React.WheelEvent) => {
    const d = Math.abs(ev.deltaX) > Math.abs(ev.deltaY) ? ev.deltaX : ev.deltaY;
    handle.pull(d * (ev.shiftKey ? 60 : 6));
    window.clearTimeout(wheelTimer.current);
    wheelTimer.current = window.setTimeout(letGo, 500);
  };
  const wheelTimer = useRef(0);

  useEffect(() => {
    const key = (ev: KeyboardEvent) => {
      if (ev.key === "ArrowLeft") handle.pull(ev.shiftKey ? -H : -5 * M);
      else if (ev.key === "ArrowRight") {
        handle.pull(ev.shiftKey ? H : 5 * M);
        letGo();
      } else return;
      ev.preventDefault();
    };
    window.addEventListener("keydown", key);
    return () => window.removeEventListener("keydown", key);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [handle]);

  return (
    <div ref={root} className="ins">
      <canvas
        ref={canvas}
        className="ins__field"
        aria-label="Ghost's field. Drag across it, scroll, or use the arrow keys to move back through the day."
        role="img"
        onPointerDown={onPointerDown}
        onPointerMove={onPointerMove}
        onPointerUp={onPointerUp}
        onPointerCancel={onPointerUp}
        onWheel={onWheel}
      />
      <TimeReadout />
      <Caption L={L} />
      <Front L={L} />
    </div>
  );
}

// ---------------------------------------------------------------- DOM over the field

function TimeReadout() {
  const { handle } = useFrame();
  const minute = useKey((h) => `${h.e.mode}|${clock(h.e.view, h.e.mode !== "live")}`);
  const [mode, text] = minute.split("|");
  return (
    <div className={mode === "live" ? "ins-time" : "ins-time is-past"} aria-live="polite">
      <span className="ins-time__clock">{text}</span>
      <span className="ins-time__note">
        {mode === "live" ? "now" : mode === "tour" ? "the day so far" : "replaying"}
        {mode === "pulled" && (
          <button type="button" className="ins-time__now" onClick={() => handle.toLive()}>
            Back to now
          </button>
        )}
      </span>
    </div>
  );
}

/** The words being said, where the voice is: under the core. */
function Caption({ L }: { L: Layout }) {
  const key = useKey((h) => {
    const s = h.state();
    if (!s.utter) return "";
    const n = h.e.reduced ? s.utter.text.length : Math.round(s.utter.text.length * Math.min(1, s.said * 1.04));
    return `${s.utter.id}|${n}`;
  });
  const { handle } = useFrame();
  if (!key) return <div className="ins-caption" aria-live="polite" style={{ top: L.captionY }} />;
  const [id, n] = key.split("|");
  const u = handle.e.day.utters.find((x) => x.id === id)!;
  const said = u.text.slice(0, Number(n));
  const rest = u.text.slice(Number(n));
  return (
    <div className={u.who === "you" ? "ins-caption is-you" : "ins-caption"} style={{ top: L.captionY, maxWidth: Math.min(L.w - 32, 760) }}>
      <span className="ins-caption__who">{u.who === "you" ? "You" : agentOf(u.who).name}</span>
      <span className="sr-only">{u.text}</span>
      <span aria-hidden>
        <span className="ins-caption__said">{said}</span>
        {u.who !== "you" && <span className="ins-caption__rest">{rest}</span>}
      </span>
    </div>
  );
}

/** What needs you, risen to the front. */
function Front({ L }: { L: Layout }) {
  const { handle } = useFrame();
  const key = useKey((h) => {
    const s = h.state();
    return `${h.e.mode === "live" ? 1 : 0}|${s.waiting.map((a) => a.id).join(",")}`;
  });
  const [liveFlag, ids] = key.split("|");
  const live = liveFlag === "1";
  const items = ids ? ids.split(",").map((id) => handle.e.day.asks.find((a) => a.id === id)!) : [];

  useEffect(() => {
    const k = (ev: KeyboardEvent) => {
      if (!live || ev.metaKey || ev.ctrlKey || ev.altKey) return;
      const d = items.find((a) => a.kind === "decision");
      const q = items.find((a) => a.kind === "question");
      if (d && (ev.key === "y" || ev.key === "Y")) handle.answer(d.id, "Yes");
      else if (d && (ev.key === "n" || ev.key === "N")) handle.answer(d.id, "No");
      else if (q && /^[1-4]$/.test(ev.key) && q.choices[Number(ev.key) - 1]) handle.answer(q.id, q.choices[Number(ev.key) - 1]);
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [items, live, handle]);

  return (
    <div className="ins-front" role="region" aria-label="Needs you">
      {items.map((a, i) => (
        <FrontItem key={a.id} a={a} x={slotX(L, i, items.length)} y={L.slotY} live={live} small={L.small} />
      ))}
    </div>
  );
}

function FrontItem({ a, x, y, live, small }: { a: Ask; x: number; y: number; live: boolean; small: boolean }) {
  const { handle } = useFrame();
  const who = a.agent === "ghost" ? "Ghost" : agentOf(a.agent).name;
  return (
    <div className={`ins-item ins-item--${a.kind}`} style={{ left: x, top: y, width: small ? "min(300px, calc(100vw - 32px))" : 380 }}>
      <div className="ins-item__who">
        {a.kind === "decision" ? `${who} will do this on your yes` : `${who} asks`} · {clock(a.t)}
      </div>
      <div className="ins-item__text">{a.text}</div>
      <div className="ins-item__choices">
        {a.choices.map((c, i) => (
          <button key={c} type="button" disabled={!live} className="ins-item__choice" onClick={() => handle.answer(a.id, c)}>
            <kbd aria-hidden>{a.kind === "decision" ? c[0] : i + 1}</kbd>
            {c}
          </button>
        ))}
        {!live && <span className="ins-item__past">waiting then</span>}
      </div>
    </div>
  );
}

// ---------------------------------------------------------------- the field

/**
 * Glows are drawn from a sprite made once per colour: a radial gradient per packet per frame
 * cost a busy frame its 16 ms at 2x density.
 */
const sprites = new Map<string, HTMLCanvasElement>();
function glow(ctx: CanvasRenderingContext2D, color: string, x: number, y: number, r: number, a: number, hot: boolean) {
  const key = `${color}|${hot}`;
  let sp = sprites.get(key);
  if (!sp) {
    sp = document.createElement("canvas");
    sp.width = sp.height = 128;
    const g2 = sp.getContext("2d")!;
    const g = g2.createRadialGradient(64, 64, 0, 64, 64, 64);
    g.addColorStop(0, alpha(color, 1));
    if (hot) g.addColorStop(0.35, alpha(color, 0.47));
    g.addColorStop(1, alpha(color, 0));
    g2.fillStyle = g;
    g2.fillRect(0, 0, 128, 128);
    sprites.set(key, sp);
  }
  ctx.globalAlpha = Math.max(0, Math.min(1, a));
  ctx.drawImage(sp, x - r, y - r, r * 2, r * 2);
  ctx.globalAlpha = 1;
}

function draw(
  ctx: CanvasRenderingContext2D,
  L: Layout,
  p: Palette,
  s: State,
  t: number,
  live: number,
  past: boolean,
  reduced: boolean,
  glyph: Path2D,
  perAgent: Map<AgentId, Call[]>,
  day: Day,
  wall: number,
) {
  const perMinute = day.perMinute;
  const { w, h, cx, cy, S, core } = L;
  // The ground is the page's own background; clearing is cheaper than painting it.
  ctx.clearRect(0, 0, w, h);

  // The ground: a faint well of light at the core, brighter the more is going on.
  const busy = Math.min(1, (s.active.length + s.running.length * 2) / 10);
  const well = ctx.createRadialGradient(cx, cy, 0, cx, cy, S * 1.4);
  well.addColorStop(0, alpha(p.h0, 0.07 + busy * 0.08));
  well.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = well;
  ctx.fillRect(cx - S * 1.4, cy - S * 1.4, S * 2.8, S * 2.8);

  // Orbits.
  ctx.lineWidth = 1;
  for (const a of AGENTS) {
    if (!a.orbit) continue;
    ctx.strokeStyle = alpha(p.text3, 0.07);
    ctx.beginPath();
    ctx.ellipse(cx, cy, a.orbit * S * 1.25, a.orbit * S * 1.25 * 0.86, 0, 0, TAU);
    ctx.stroke();
  }

  // Apps on the outer ring, lit by use.
  ctx.font = `500 ${L.small ? 9 : 10.5}px "JetBrains Mono Variable", ui-monospace, monospace`;
  ctx.textBaseline = "middle";
  for (const app of APPS) {
    const pos = appPos(L, app.id);
    const act = s.active.filter((c) => c.app === app.id).length;
    const rec = s.recent.filter((c) => c.app === app.id).reduce((m, c) => m + Math.max(0, 1 - (t - c.t - c.dur) / 10), 0);
    const lit = Math.min(1, act * 0.5 + rec * 0.25);
    if (lit > 0.02) {
      glow(ctx, p.text1, pos.x, pos.y, 26 + lit * 30, 0.18 * lit, false);
    }
    ctx.strokeStyle = alpha(p.text2, 0.35 + lit * 0.6);
    ctx.lineWidth = 1.25;
    ctx.beginPath();
    ctx.arc(pos.x, pos.y, 4.5 + lit * 2, 0, TAU);
    ctx.stroke();
    const right = Math.cos(app.angle) >= -0.1;
    ctx.textAlign = right ? "left" : "right";
    ctx.fillStyle = alpha(p.text2, 0.45 + lit * 0.55);
    const lx = pos.x + (right ? 12 : -12);
    ctx.fillText(app.name.toUpperCase(), lx, pos.y);
    // What just came back, at the app: type where the action is.
    const last = [...s.active, ...s.recent].filter((c) => c.app === app.id).pop();
    if (last && !L.small) {
      const age = t - (last.t + last.dur);
      const a = age < 0 ? 0.9 : Math.max(0, 0.9 - age / 4);
      if (a > 0.02) {
        ctx.fillStyle = alpha(p.text3, a);
        ctx.fillText(`${last.tool}${last.read ? ` · ${last.items} · ${size(last.bytes)}` : " · change"}`, lx, pos.y + 14);
      }
    }
  }

  // Agents: trails of where they were when they worked, then the bodies.
  const reachOf = new Map<AgentId, { x: number; y: number }>();
  for (const a of AGENTS) {
    if (!a.orbit) continue;
    const calls = perAgent.get(a.id)!;
    const color = hueOf(p, a.id);
    // The orbit behind the body, brighter where work was done.
    const pos = agentPos(L, a.id, t);
    reachOf.set(a.id, pos);
    const r = a.orbit * S * 1.25;
    const span = 2 * H;
    ctx.lineWidth = 1.5;
    for (let k = 0; k < 24; k++) {
      const t0 = t - (k / 24) * span;
      const t1 = t - ((k + 1) / 24) * span;
      if (t1 < DAY_START) break;
      const a0 = a.phase0 + a.speed * TAU * (t0 / H);
      const a1 = a.phase0 + a.speed * TAU * (t1 / H);
      ctx.strokeStyle = alpha(color, 0.22 * (1 - k / 24));
      ctx.beginPath();
      ctx.ellipse(cx, cy, r, r * 0.86, 0, Math.min(a0, a1), Math.max(a0, a1));
      ctx.stroke();
    }
    for (const c of calls) {
      if (c.t > t) break;
      const age = t - c.t;
      if (age > span) continue;
      const cp = agentPos(L, a.id, c.t);
      ctx.fillStyle = alpha(color, 0.75 * (1 - age / span) + 0.12);
      const rr = 1.2 + Math.sqrt(c.bytes) / 45;
      ctx.beginPath();
      ctx.arc(cp.x, cp.y, rr, 0, TAU);
      ctx.fill();
    }
    const run = s.running.find((x) => x.agent === a.id);
    const pulse = run ? 0.5 + 0.5 * Math.sin((reduced ? 0 : wall) * 6) : 0;
    const br = (L.small ? 5 : 7) + (run ? 3 + pulse * 2 : 0);
    if (run) {
      glow(ctx, color, pos.x, pos.y, br * 5, 0.35, false);
    }
    ctx.fillStyle = color;
    ctx.beginPath();
    ctx.arc(pos.x, pos.y, br, 0, TAU);
    ctx.fill();
    ctx.fillStyle = p.ground;
    ctx.beginPath();
    ctx.arc(pos.x, pos.y, br * 0.45, 0, TAU);
    ctx.fill();
    ctx.font = `500 ${L.small ? 10 : 12}px "Inter Variable", system-ui, sans-serif`;
    ctx.textAlign = "left";
    ctx.fillStyle = alpha(p.text1, run ? 0.95 : 0.6);
    ctx.fillText(a.name, pos.x + br + 6, pos.y - 1);
    if (run && !L.small) {
      const done = s.recent.filter((c) => c.agent === a.id).length + s.active.filter((c) => c.agent === a.id).length;
      ctx.font = `400 10.5px "JetBrains Mono Variable", ui-monospace, monospace`;
      ctx.fillStyle = alpha(color, 0.9);
      ctx.fillText(`${run.schedule.toLowerCase()} · working · ${done} calls`, pos.x + br + 6, pos.y + 13);
    }
  }

  // Calls: out as a spark, back as light sized by what came back.
  for (const c of [...s.active, ...s.recent]) {
    const from = c.agent === "ghost" ? { x: cx, y: cy } : (reachOf.get(c.agent) ?? agentPos(L, c.agent, t));
    const to = appPos(L, c.app);
    const k = (t - c.t) / c.dur;
    const color = c.read ? hueOf(p, c.agent) : p.text1;
    const fade = k <= 1 ? 1 : Math.max(0, 1 - (t - c.t - c.dur) / 3);
    if (fade <= 0) continue;
    // The path.
    const mid = bez(from.x, from.y, to.x, to.y, L, 0.5);
    ctx.strokeStyle = alpha(color, (k <= 1 ? 0.28 : 0.12) * fade);
    ctx.lineWidth = k <= 1 ? 1.2 : 0.8;
    ctx.beginPath();
    ctx.moveTo(from.x, from.y);
    ctx.quadraticCurveTo(mid.qx, mid.qy, to.x, to.y);
    ctx.stroke();
    if (k > 1) continue;
    if (reduced) {
      const q = bez(from.x, from.y, to.x, to.y, L, 0.5);
      ctx.fillStyle = alpha(color, 0.9);
      ctx.beginPath();
      ctx.arc(q.x, q.y, 2 + Math.sqrt(c.bytes) / 40, 0, TAU);
      ctx.fill();
      continue;
    }
    if (k < 0.45) {
      const q = bez(from.x, from.y, to.x, to.y, L, k / 0.45);
      ctx.fillStyle = alpha(p.text1, 0.95);
      ctx.beginPath();
      ctx.arc(q.x, q.y, 1.6, 0, TAU);
      ctx.fill();
    } else if (k > 0.55) {
      const kk = 1 - (k - 0.55) / 0.45;
      const q = bez(from.x, from.y, to.x, to.y, L, kk);
      const rad = 2 + Math.sqrt(c.bytes) / 22;
      glow(ctx, color, q.x, q.y, rad * 3, 0.95, true);
      // a short comet tail back along the path
      for (let j = 1; j <= 5; j++) {
        const qq = bez(from.x, from.y, to.x, to.y, L, Math.min(1, kk + j * 0.025));
        ctx.fillStyle = alpha(color, 0.3 * (1 - j / 6));
        ctx.beginPath();
        ctx.arc(qq.x, qq.y, rad * (1 - j / 7), 0, TAU);
        ctx.fill();
      }
    }
  }

  // Your voice, streaming in from where you sit (the bottom edge) to the core.
  if (s.phase === "you" && s.utter) {
    const u = s.utter;
    const N = reduced ? 0 : 90;
    for (let i = 0; i < N; i++) {
      const seed = (i * 0.6180339) % 1;
      const ph = (seed + (t - u.t) * 0.85) % 1;
      const lv = levelAt(u, t - (1 - ph) * 0.7);
      if (lv < 0.05) continue;
      const sx = cx + (seed - 0.5) * Math.min(w * 0.7, 700);
      const sy = h + 10;
      const k = ph * ph;
      const x = sx + (cx - sx) * k;
      const y = sy + (cy + core - sy) * k + Math.sin(ph * 9 + i) * 6 * (1 - k);
      ctx.fillStyle = alpha(p.listening, lv * 0.85 * (0.3 + k));
      ctx.beginPath();
      ctx.arc(x, y, 1 + lv * 2.4 * (1 - k * 0.6), 0, TAU);
      ctx.fill();
    }
    if (reduced) {
      ctx.strokeStyle = alpha(p.listening, 0.35 + s.level * 0.5);
      ctx.lineWidth = 2;
      ctx.beginPath();
      ctx.moveTo(cx, h);
      ctx.lineTo(cx, cy + core * 1.6);
      ctx.stroke();
    }
  }

  // The core.
  const phaseColor = s.phase === "you" ? p.listening : s.phase === "thinking" ? p.thinking : s.phase === "speaking" ? p.speaking : s.phase === "listening" ? p.listening : p.text2;
  const breathe = s.phase === "passive" && !reduced ? 0.5 + 0.5 * Math.sin(wall * 1.1) : 0.5;
  const halo = core * (1.9 + s.level * 1.6 + busy * 0.6 + breathe * 0.2);
  const hg = ctx.createRadialGradient(cx, cy, core * 0.4, cx, cy, halo);
  hg.addColorStop(0, alpha(phaseColor, s.phase === "passive" ? 0.16 : 0.32));
  hg.addColorStop(1, "rgba(0,0,0,0)");
  ctx.fillStyle = hg;
  ctx.beginPath();
  ctx.arc(cx, cy, halo, 0, TAU);
  ctx.fill();

  // Ghost's voice leaving as rings: each ring is the level a moment ago.
  if (s.phase === "speaking" && s.utter) {
    const u = s.utter;
    const rings = reduced ? 1 : 7;
    for (let i = 0; i < rings; i++) {
      const lag = i * 0.11;
      const lv = levelAt(u, t - lag);
      const base = core * 1.55 + i * core * 0.42;
      ctx.strokeStyle = alpha(p.speaking, (0.75 - i * 0.09) * (0.25 + lv));
      ctx.lineWidth = 1.4;
      ctx.beginPath();
      for (let j = 0; j <= 96; j++) {
        const ang = (j / 96) * TAU;
        const wob = reduced ? 0 : lv * core * 0.18 * Math.sin(ang * 6 + (t - lag) * 9 + i) * Math.sin(ang * 3 - (t - lag) * 4);
        const rr = base + lv * core * 0.25 + wob;
        const x = cx + Math.cos(ang) * rr;
        const y = cy + Math.sin(ang) * rr;
        if (j) ctx.lineTo(x, y);
        else ctx.moveTo(x, y);
      }
      ctx.stroke();
    }
  }
  // Thinking: arcs turning, one per call in flight.
  if (s.phase === "thinking" || (s.session && s.active.some((c) => c.agent === "ghost"))) {
    const n = Math.max(1, s.active.filter((c) => c.agent === "ghost").length);
    const rot = reduced ? 0 : t * 2.2;
    ctx.lineWidth = 2;
    ctx.strokeStyle = alpha(p.thinking, 0.85);
    for (let i = 0; i < n; i++) {
      const a0 = rot + (i / n) * TAU;
      ctx.beginPath();
      ctx.arc(cx, cy, core * 1.55, a0, a0 + (TAU / n) * 0.55);
      ctx.stroke();
    }
  }
  // Listening in a session: a quiet ring.
  if (s.phase === "listening") {
    ctx.strokeStyle = alpha(p.listening, 0.5);
    ctx.lineWidth = 1;
    ctx.beginPath();
    ctx.arc(cx, cy, core * 1.55, 0, TAU);
    ctx.stroke();
  }

  // Wisp itself.
  ctx.save();
  const scale = (core * 1.5) / 24;
  const lift = s.phase === "you" || s.phase === "speaking" ? -s.level * core * 0.06 : 0;
  ctx.translate(cx - 12 * scale, cy - 12.5 * scale + lift);
  ctx.scale(scale, scale);
  ctx.fillStyle = s.phase === "passive" ? alpha(p.text1, 0.78) : p.text1;
  ctx.fill(glyph, "nonzero");
  ctx.restore();

  // What needs you: a thread from who asked to where it waits at the front.
  for (let i = 0; i < s.waiting.length; i++) {
    const a = s.waiting[i];
    const from = a.agent === "ghost" ? { x: cx, y: cy + core } : agentPos(L, a.agent, t);
    const sx = slotX(L, i, s.waiting.length);
    const rise = reduced ? 1 : Math.min(1, (t - a.t) / 1.2);
    const ease = 1 - Math.pow(1 - rise, 3);
    const tx = from.x + (sx - from.x) * ease;
    const ty = from.y + (L.slotY - from.y) * ease;
    const color = a.kind === "decision" ? p.warn : hueOf(p, a.agent);
    ctx.strokeStyle = alpha(color, 0.35);
    ctx.lineWidth = 1;
    ctx.setLineDash([2, 4]);
    ctx.beginPath();
    ctx.moveTo(from.x, from.y);
    ctx.lineTo(tx, ty);
    ctx.stroke();
    ctx.setLineDash([]);
    const beat = reduced ? 0.5 : 0.5 + 0.5 * Math.sin(wall * 2.4 + i);
    glow(ctx, color, tx, ty, 34 + beat * 10, 0.5, false);
  }

  // The day: the axis you pull, with how much work each minute held.
  const ax0 = 16;
  const ax1 = w - 16;
  const span = live - DAY_START;
  const X = (tt: number) => ax0 + ((tt - DAY_START) / span) * (ax1 - ax0);
  const y0 = L.axisY;
  ctx.strokeStyle = alpha(p.text3, 0.35);
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(ax0, y0);
  ctx.lineTo(ax1, y0);
  ctx.stroke();
  const n = Math.ceil(span / M);
  for (let i = 0; i < n && i < perMinute.length; i++) {
    const v = perMinute[i];
    if (!v) continue;
    const x = X(DAY_START + i * M);
    ctx.strokeStyle = alpha(p.text2, Math.min(0.9, 0.25 + v * 0.06));
    ctx.beginPath();
    ctx.moveTo(x, y0);
    ctx.lineTo(x, y0 - Math.min(14, 2 + v * 0.6));
    ctx.stroke();
  }
  ctx.font = `400 ${L.small ? 9 : 10}px "JetBrains Mono Variable", ui-monospace, monospace`;
  ctx.textAlign = "center";
  ctx.textBaseline = "top";
  for (let hh = Math.ceil(DAY_START / H); hh * H <= live; hh++) {
    const x = X(hh * H);
    ctx.fillStyle = alpha(p.text3, 0.8);
    if (!L.small || hh % 2 === 0) ctx.fillText(String(hh).padStart(2, "0"), x, y0 + 5);
  }
  // Where you spoke, and where something needed you.
  for (const se of day.sessions) {
    if (se.t > live) continue;
    ctx.fillStyle = alpha(p.listening, 0.8);
    ctx.fillRect(X(se.t), y0 - 1, Math.max(2, X(Math.min(live, se.t + se.dur)) - X(se.t)), 3);
  }
  for (const a of day.asks) {
    if (a.t > live) continue;
    ctx.fillStyle = a.kind === "decision" ? p.warn : hueOf(p, a.agent);
    ctx.beginPath();
    ctx.arc(X(a.t), y0 - 22, 2.5, 0, TAU);
    ctx.fill();
  }
  // The view.
  const vx = X(t);
  ctx.strokeStyle = past ? p.text1 : alpha(p.listening, 0.9);
  ctx.lineWidth = 2;
  ctx.beginPath();
  ctx.moveTo(vx, y0 - 18);
  ctx.lineTo(vx, y0 + 3);
  ctx.stroke();
  if (past) {
    ctx.strokeStyle = alpha(p.text1, 0.18);
    ctx.lineWidth = 6;
    ctx.beginPath();
    ctx.moveTo(vx, y0);
    ctx.lineTo(ax1, y0);
    ctx.stroke();
  }

  // Runs that finished in the last few seconds: a ring off the body and a line of type.
  drawFinished(ctx, L, p, t, reduced, day);
}

/** Found from the day, not the state, so a replay shows them too. */
function drawFinished(ctx: CanvasRenderingContext2D, L: Layout, p: Palette, t: number, reduced: boolean, day: Day) {
  for (const r of day.runs) {
    const end = r.t + r.dur;
    const age = t - end;
    if (age < 0 || age > 9) continue;
    const pos = agentPos(L, r.agent, t);
    const color = hueOf(p, r.agent);
    if (!reduced) {
      for (let i = 0; i < 2; i++) {
        const k = (age - i * 0.4) / 2.2;
        if (k < 0 || k > 1) continue;
        ctx.strokeStyle = alpha(color, 0.7 * (1 - k));
        ctx.lineWidth = 1.5;
        ctx.beginPath();
        ctx.arc(pos.x, pos.y, 8 + k * 70, 0, TAU);
        ctx.stroke();
      }
      // The answer going to the core (the waiting list).
      const k = Math.min(1, age / 1.6);
      if (k < 1) {
        const x = pos.x + (L.cx - pos.x) * k * k;
        const y = pos.y + (L.cy - pos.y) * k * k;
        ctx.fillStyle = alpha(color, 0.9);
        ctx.beginPath();
        ctx.arc(x, y, 3, 0, TAU);
        ctx.fill();
      }
    }
    const reads = day.calls.filter((c) => c.agent === r.agent && c.t >= r.t && c.t <= end);
    const bytes = reads.reduce((m, c) => m + c.bytes, 0);
    const a = Math.min(1, (9 - age) / 2);
    ctx.textAlign = pos.x > L.cx ? "left" : "right";
    ctx.textBaseline = "middle";
    const dx = pos.x > L.cx ? 16 : -16;
    ctx.font = `500 ${L.small ? 11 : 13}px "Inter Variable", system-ui, sans-serif`;
    ctx.fillStyle = alpha(p.text1, a);
    const head = `${agentOf(r.agent).name} finished · ${reads.length} calls · ${size(bytes)}`;
    ctx.fillText(head, pos.x + dx, pos.y + 22);
    if (!L.small) {
      ctx.font = `400 12px "Inter Variable", system-ui, sans-serif`;
      ctx.fillStyle = alpha(p.text2, a);
      const words = r.answer.length > 64 ? `${r.answer.slice(0, 62)}…` : r.answer;
      ctx.fillText(words, pos.x + dx, pos.y + 40);
    }
  }
}
