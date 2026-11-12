/**
 * Owner: design. Concept: the signal. No objects, only language and time. One living line of
 * very large type is what is being said now, by Ghost or by you; the lines before it are the
 * river of the day, which you scroll like a seismograph. Under the line runs the signal: one
 * hairline that becomes the voice and spikes with every call. The machine's work runs in a
 * dense margin of telemetry. Decisions are one word, pressed or said.
 *
 * DOM type over two small canvases (the trace and the day's seismograph). No dependency.
 */
import { useEffect, useMemo, useRef } from "react";

import { agentOf, AGENTS, type AgentId, type Answer, answerOf, type Ask, type Call, clock, type Day, DAY_START, H, levelAt, M, size, type Who } from "./day";
import { alpha, fitCanvas, resolveColors, useFrame, useKey } from "./shared";

interface Line {
  id: string;
  t: number;
  /** When the line is whole: the end of the utterance, or `t`. */
  end: number;
  who: Who;
  kind: "said" | "run" | "ask" | "answer" | "change";
  text: string;
  label: string;
  ask?: Ask;
}

const nameOf = (w: Who) => (w === "you" ? "You" : agentOf(w).name);

/** Every line of the day up to `live`, in order, answers included. */
function linesOf(day: Day, answers: Map<string, Answer>): Line[] {
  const out: Line[] = [];
  for (const u of day.utters) out.push({ id: u.id, t: u.t, end: u.t + u.dur, who: u.who, kind: "said", text: u.text, label: u.who === "you" ? "You said" : `${nameOf(u.who)} said` });
  for (const r of day.runs) out.push({ id: `run-${r.id}`, t: r.t + r.dur, end: r.t + r.dur, who: r.agent, kind: "run", text: r.answer, label: `${nameOf(r.agent)} · ${r.schedule.toLowerCase()}` });
  for (const a of day.asks) {
    out.push({ id: `ask-${a.id}`, t: a.t, end: a.t, who: a.agent, kind: "ask", text: a.text, label: a.kind === "decision" ? `${nameOf(a.agent)} · on your yes` : `${nameOf(a.agent)} asks`, ask: a });
    const ans = answerOf(a, answers);
    if (ans) out.push({ id: `ans-${a.id}`, t: ans.at, end: ans.at, who: "you", kind: "answer", text: ans.choice + ".", label: ans.how === "voice" ? "You answered, aloud" : "You answered" });
  }
  for (const c of day.changes) out.push({ id: `chg-${c.t}`, t: c.t, end: c.t, who: c.agent, kind: "change", text: c.text + ".", label: `${nameOf(c.agent)} · done` });
  return out.sort((a, b) => a.t - b.t);
}

const COLORS = {
  text1: "var(--text-1)",
  text2: "var(--text-2)",
  text3: "var(--text-3)",
  warn: "var(--warn)",
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
const hueOf = (p: Palette, id: AgentId) => p[`h${agentOf(id).hue}` as keyof Palette];

export default function Signal() {
  const { handle, onFrame } = useFrame();
  const root = useRef<HTMLDivElement>(null);
  const palette = useRef<Palette | null>(null);
  useEffect(() => {
    palette.current = resolveColors(root.current!, COLORS);
  }, []);

  // Lines change only when an answer is given.
  const version = useKey((h) => h.e.version);
  const lines = useMemo(() => linesOf(handle.e.day, handle.e.answers), [handle, version]);

  // ------------------------------------------------------------ moving through the day
  const step = (dir: -1 | 1) => {
    const e = handle.e;
    const cur = currentIndex(lines, e.view);
    const next = cur + dir;
    if (dir > 0 && (next >= lines.length || lines[next].t > e.live)) return handle.toLive();
    if (next < 0) return;
    const l = lines[next];
    const after = lines[next + 1];
    handle.seek(Math.min(l.end, after ? after.t - 0.01 : l.end, e.live));
    if (e.live - e.view < 1.5) handle.toLive();
  };
  const wheelAcc = useRef(0);
  const onWheel = (ev: React.WheelEvent) => {
    wheelAcc.current += ev.deltaY;
    if (ev.shiftKey && Math.abs(ev.deltaY) > 0) {
      handle.pull(Math.sign(ev.deltaY) * 10 * M);
      wheelAcc.current = 0;
      return;
    }
    while (Math.abs(wheelAcc.current) >= 60) {
      const d = wheelAcc.current > 0 ? 1 : -1;
      step(d);
      wheelAcc.current -= d * 60;
    }
  };
  const drag = useRef<number | null>(null);
  const onPointerDown = (ev: React.PointerEvent) => {
    if ((ev.target as HTMLElement).closest("button")) return;
    drag.current = ev.clientY;
    (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
  };
  const onPointerMove = (ev: React.PointerEvent) => {
    if (drag.current === null) return;
    const dy = ev.clientY - drag.current;
    if (Math.abs(dy) >= 44) {
      step(dy > 0 ? -1 : 1);
      drag.current = ev.clientY;
    }
  };
  const onPointerUp = () => {
    drag.current = null;
  };

  useEffect(() => {
    const k = (ev: KeyboardEvent) => {
      if (ev.key === "ArrowUp") ev.shiftKey ? handle.pull(-H) : step(-1);
      else if (ev.key === "ArrowDown") {
        if (ev.shiftKey) {
          handle.pull(H);
          if (handle.e.live - handle.e.view < 1.5) handle.toLive();
        } else step(1);
      } else if (ev.key === "PageUp") handle.pull(-H);
      else if (ev.key === "PageDown") handle.pull(H);
      else return;
      ev.preventDefault();
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  });

  return (
    <div ref={root} className="sig">
      <Seismograph palette={palette} />
      <main className="sig-river" onWheel={onWheel} onPointerDown={onPointerDown} onPointerMove={onPointerMove} onPointerUp={onPointerUp} onPointerCancel={onPointerUp}>
        <Head />
        <River lines={lines} />
        <Trace palette={palette} onFrame={onFrame} />
        <Decide />
      </main>
      <Telemetry />
    </div>
  );
}

function currentIndex(lines: Line[], t: number): number {
  let i = -1;
  for (let k = 0; k < lines.length; k++) {
    if (lines[k].t <= t) i = k;
    else break;
  }
  return i;
}

// ---------------------------------------------------------------- the head: where you are

function Head() {
  const { handle } = useFrame();
  const key = useKey((h) => `${h.e.mode}|${clock(h.e.view, true)}|${h.state().phase}`);
  const [mode, time, phase] = key.split("|");
  const word: Record<string, string> = { passive: "Waiting for its name", you: "Hearing you", thinking: "Working", speaking: "Speaking", listening: "Listening" };
  return (
    <header className="sig-head">
      <span className="sig-head__time">{time}</span>
      <span className={`sig-head__state is-${phase}`}>{word[phase]}</span>
      <span className="sig-head__mode">
        {mode === "live" ? (
          "Now"
        ) : (
          <>
            {mode === "tour" ? "The day so far" : "Replay"}{" "}
            <button type="button" className="sig-word sig-word--small" onClick={() => handle.toLive()}>
              Now
            </button>
          </>
        )}
      </span>
    </header>
  );
}

// ---------------------------------------------------------------- the river

function River({ lines }: { lines: Line[] }) {
  const { reduced } = useFrame();
  const key = useKey((h) => {
    const t = h.e.view;
    const i = currentIndex(lines, t);
    if (i < 0) return "-1|0";
    const l = lines[i];
    if (l.kind === "said") {
      const said = Math.min(1, (t - l.t) / Math.max(0.01, l.end - l.t));
      return `${i}|${h.e.reduced ? l.text.length : Math.round(l.text.length * Math.min(1, said * 1.04))}`;
    }
    return `${i}|${l.text.length}`;
  });
  const [is, ns] = key.split("|");
  const i = Number(is);
  const n = Number(ns);
  const shown = lines.slice(Math.max(0, i - 7), i + 1);
  return (
    <div className="sig-lines" aria-live="polite">
      {shown.map((l, k) => {
        const d = shown.length - 1 - k;
        const now = d === 0;
        const said = now ? l.text.slice(0, n) : l.text;
        const rest = now ? l.text.slice(n) : "";
        return (
          <div key={l.id} className={`sig-line sig-line--d${Math.min(d, 4)}${l.text.length > 72 ? " sig-line--long" : ""} sig-line--${l.kind} sig-line--${l.who === "you" ? "you" : "machine"}`} data-reduced={reduced || undefined}>
            <div className="sig-line__meta">
              <span>{clock(l.t, true)}</span>
              <span className={`sig-line__who sig-hue-${l.who === "you" ? "you" : agentOf(l.who).hue}`}>{l.label}</span>
            </div>
            <p className="sig-line__text">
              <span className="sr-only">{l.text}</span>
              <span aria-hidden>
                {said}
                {l.who !== "you" && rest && <span className="sig-line__rest">{rest}</span>}
                {now && l.who === "you" && rest && <span className="sig-caret" />}
              </span>
            </p>
          </div>
        );
      })}
    </div>
  );
}

// ---------------------------------------------------------------- the signal

/** The hairline under the line: the last twelve seconds, your voice above, Ghost's mirrored, calls as spikes. */
function Trace({ palette, onFrame }: { palette: React.RefObject<Palette | null>; onFrame: (f: (now: number) => void) => () => void }) {
  const { handle, reduced } = useFrame();
  const c = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const ctx = c.current!.getContext("2d")!;
    let lastDrawn = -1;
    return onFrame(() => {
      const p = palette.current;
      if (!p) return;
      const e = handle.e;
      const t = e.view;
      if (reduced && Math.abs(t - lastDrawn) < 0.5) return;
      lastDrawn = t;
      const { w, h, dpr } = fitCanvas(c.current!);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const win = 14;
      const mid = h * 0.55;
      const X = (tt: number) => w - ((t - tt) / win) * w;
      // the hairline
      ctx.fillStyle = alpha(p.text1, 0.5);
      ctx.fillRect(0, mid, w, 1);
      // voices
      const us = e.day.utters.filter((u) => u.t < t && u.t + u.dur > t - win);
      for (const u of us) {
        const you = u.who === "you";
        ctx.fillStyle = you ? p.text1 : p.speaking;
        for (let x = Math.max(0, X(u.t)); x < Math.min(w, X(Math.min(t, u.t + u.dur))); x += 2) {
          const tt = t - (1 - x / w) * win;
          const lv = levelAt(u, tt);
          const a = lv * mid * 0.9;
          if (you) ctx.fillRect(x, mid - a, 1, a);
          else ctx.fillRect(x, mid - a * 0.6, 1, a * 1.2);
        }
      }
      // thinking: a dotted run along the line
      for (const s of e.day.thinks) {
        if (s.t > t || s.t + s.dur < t - win) continue;
        ctx.fillStyle = p.thinking;
        for (let x = Math.max(0, X(s.t)); x < Math.min(w, X(Math.min(t, s.t + s.dur))); x += 5) ctx.fillRect(x, mid - 1, 2, 3);
      }
      // calls: a spike out, sized by what came back
      for (const call of e.day.calls) {
        if (call.t > t) break;
        if (call.t + call.dur < t - win) continue;
        const x0 = X(call.t);
        const x1 = X(Math.min(t, call.t + call.dur));
        const color = call.read ? hueOf(p, call.agent) : p.text1;
        const hgt = Math.min(h - mid - 2, 4 + Math.sqrt(call.bytes) / 7);
        ctx.fillStyle = alpha(color, 0.35);
        ctx.fillRect(x0, mid + 2, Math.max(1, x1 - x0), 2);
        if (call.t + call.dur <= t) {
          ctx.fillStyle = color;
          ctx.fillRect(x1 - 1, mid + 2, 2, hgt);
        }
      }
      // now
      ctx.fillStyle = e.mode === "live" ? p.listening : p.text1;
      ctx.fillRect(w - 2, 0, 2, h);
    });
  }, [handle, onFrame, palette, reduced]);
  return <canvas ref={c} className="sig-trace" aria-hidden />;
}

// ---------------------------------------------------------------- deciding: one word

function Decide() {
  const { handle } = useFrame();
  const key = useKey((h) => `${h.e.mode === "live" ? 1 : 0}|${h.state().waiting.map((a) => a.id).join(",")}`);
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
  if (!items.length) return <div className="sig-decide" />;
  return (
    <div className="sig-decide" role="region" aria-label="Needs you">
      {items.map((a) => (
        <div key={a.id} className={`sig-ask sig-ask--${a.kind}`}>
          <div className="sig-ask__q">
            <span className="sig-ask__who">{a.kind === "decision" ? `${nameOf(a.agent)} · on your yes` : `${nameOf(a.agent)} asks`}</span> {a.text}
          </div>
          <div className="sig-ask__words">
            {a.choices.map((c, i) => (
              <button key={c} type="button" className="sig-word" disabled={!live} onClick={() => handle.answer(a.id, c)} aria-keyshortcuts={a.kind === "decision" ? c[0] : String(i + 1)}>
                {c}
              </button>
            ))}
            {!live && <span className="sig-ask__then">open then</span>}
          </div>
        </div>
      ))}
    </div>
  );
}

// ---------------------------------------------------------------- telemetry

function Telemetry() {
  const { handle } = useFrame();
  const tick = useKey((h) => Math.floor(h.e.view * 8));
  void tick;
  const e = handle.e;
  const t = e.view;
  const s = handle.state();
  let reads = 0;
  let changes = 0;
  let bytes = 0;
  let last: Call[] = [];
  const minute: Call[] = [];
  for (const c of e.day.calls) {
    if (c.t > t) break;
    if (c.read) {
      reads++;
      bytes += c.bytes;
    } else changes++;
    if (t - c.t < 60) minute.push(c);
  }
  last = e.day.calls.filter((c) => c.t <= t).slice(-26);
  const runs = e.day.runs.filter((r) => r.t <= t).length;
  const turns = e.day.utters.filter((u) => u.who !== "you" && u.t <= t).length;
  const minBytes = minute.reduce((m, c) => m + c.bytes, 0);
  const ms = (c: Call) => `${Math.round(c.dur * 1000)}`.padStart(5);
  return (
    <aside className="sig-tele" aria-label="Telemetry">
      <section className="sig-tele__block">
        <div className="sig-tele__h">Today to {clock(t)}</div>
        <dl className="sig-tele__nums">
          <dt>reads</dt>
          <dd>{reads}</dd>
          <dt>changes</dt>
          <dd>{changes + e.day.changes.filter((c) => c.t <= t).length}</dd>
          <dt>in</dt>
          <dd>{size(bytes)}</dd>
          <dt>runs</dt>
          <dd>{runs}</dd>
          <dt>replies</dt>
          <dd>{turns}</dd>
          <dt>last min</dt>
          <dd>
            {minute.length} · {size(minBytes)}
          </dd>
        </dl>
      </section>
      <section className="sig-tele__block">
        <div className="sig-tele__h">Agents</div>
        {AGENTS.map((a) => {
          const run = s.running.find((r) => r.agent === a.id);
          const act = s.active.filter((c) => c.agent === a.id).length;
          const lastRun = e.day.runs.filter((r) => r.agent === a.id && r.t + r.dur <= t).pop();
          const speaking = s.utter?.who === a.id;
          const state = speaking ? "speaking" : run ? `run · ${act} open` : a.id === "ghost" && s.session ? s.phase : lastRun ? `idle ${Math.round((t - lastRun.t - lastRun.dur) / M)}m` : "idle";
          return (
            <div key={a.id} className={run || speaking || (a.id === "ghost" && s.session) ? "sig-tele__agent is-on" : "sig-tele__agent"}>
              <span className={`sig-hue-${a.hue}`}>{a.name.toUpperCase().padEnd(11)}</span>
              <span>{state}</span>
              <span className="sig-tele__bar" aria-hidden>
                {"▮".repeat(Math.min(8, act))}
                {"·".repeat(8 - Math.min(8, act))}
              </span>
            </div>
          );
        })}
      </section>
      <section className="sig-tele__block sig-tele__log">
        <div className="sig-tele__h">Calls</div>
        {last.map((c) => {
          const open = c.t + c.dur > t;
          return (
            <div key={c.id} className={open ? "sig-tele__call is-open" : c.read ? "sig-tele__call" : "sig-tele__call is-change"}>
              <span>{clock(c.t, true).slice(3)}</span>
              <span className={`sig-hue-${agentOf(c.agent).hue}`}>{c.agent.slice(0, 4)}</span>
              <span className="sig-tele__tool">
                {c.app}.{c.tool}
              </span>
              <span className="sig-tele__n">{open ? "…" : c.read ? c.items : "chg"}</span>
              <span className="sig-tele__n">{open ? "" : size(c.bytes)}</span>
              <span className="sig-tele__n">{open ? "" : ms(c)}</span>
            </div>
          );
        })}
      </section>
    </aside>
  );
}

// ---------------------------------------------------------------- the day, as a seismograph

function Seismograph({ palette }: { palette: React.RefObject<Palette | null> }) {
  const { handle, onFrame } = useFrame();
  const c = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const ctx = c.current!.getContext("2d")!;
    return onFrame(() => {
      const p = palette.current;
      if (!p) return;
      const { w, h, dpr } = fitCanvas(c.current!);
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, w, h);
      const e = handle.e;
      const top = 40;
      const bottom = h - 20;
      const Y = (tt: number) => top + ((tt - DAY_START) / (e.live - DAY_START)) * (bottom - top);
      const x0 = w - 14;
      ctx.fillStyle = alpha(p.text3, 0.6);
      ctx.fillRect(x0, top, 1, bottom - top);
      // sessions: where you spoke
      for (const s of e.day.sessions) {
        if (s.t > e.live) continue;
        ctx.fillStyle = alpha(p.listening, 0.8);
        ctx.fillRect(x0 + 3, Y(s.t), 3, Math.max(2, Y(Math.min(e.live, s.t + s.dur)) - Y(s.t)));
      }
      // the trace: calls per minute, swinging left
      ctx.strokeStyle = p.text1;
      ctx.lineWidth = 1;
      ctx.beginPath();
      const n = Math.ceil((e.live - DAY_START) / M);
      for (let i = 0; i < n; i++) {
        const v = e.day.perMinute[i] ?? 0;
        const y = Y(DAY_START + i * M);
        const x = x0 - Math.min(w - 26, v * 4.2) * (i % 2 ? 1 : 0.6);
        if (i) ctx.lineTo(x, y);
        else ctx.moveTo(x, y);
      }
      ctx.stroke();
      // asks: a tick where something needed you
      for (const a of e.day.asks) {
        if (a.t > e.live) continue;
        ctx.fillStyle = a.kind === "decision" ? p.warn : hueOf(p, a.agent);
        ctx.fillRect(x0 - 6, Y(a.t) - 1, 12, 2);
      }
      // hours
      ctx.font = `400 10px "JetBrains Mono Variable", ui-monospace, monospace`;
      ctx.fillStyle = alpha(p.text3, 0.9);
      ctx.textAlign = "left";
      ctx.textBaseline = "middle";
      for (let hh = Math.ceil(DAY_START / H); hh * H <= e.live; hh++) ctx.fillText(String(hh).padStart(2, "0"), 6, Y(hh * H));
      // where you are
      const vy = Y(e.view);
      ctx.fillStyle = e.mode === "live" ? p.listening : p.text1;
      ctx.fillRect(4, vy - 1, w - 4, 2);
    });
  }, [handle, onFrame, palette]);
  const seek = (ev: React.PointerEvent) => {
    const r = c.current!.getBoundingClientRect();
    const k = (ev.clientY - r.top - 40) / (r.height - 60);
    const e = handle.e;
    handle.seek(DAY_START + Math.max(0, Math.min(1, k)) * (e.live - DAY_START));
    if (e.live - e.view < 60) handle.toLive();
  };
  return (
    <canvas
      ref={c}
      className="sig-seis"
      role="img"
      aria-label="The day as a seismograph: drag along it to move through the day."
      onPointerDown={(ev) => {
        (ev.currentTarget as HTMLElement).setPointerCapture(ev.pointerId);
        seek(ev);
      }}
      onPointerMove={(ev) => ev.buttons && seek(ev)}
    />
  );
}
