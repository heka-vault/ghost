/**
 * The floating room (ADR 0059; docs/design-system.md, The floating room): the room posture
 * without a window. At rest a thin line of light at the bottom edge of a monitor (a struck red
 * microphone when Ghost isn't listening, ADR 0057); when a spoken session at this desk floats
 * (`postureOf`), Ghost comes forward from depth over the desktop: the reply in very large type,
 * the operator's last words above it, the session's real work in a dense margin, and a change
 * waiting for a yes or an agent's question with its buttons. When the session ends it falls
 * back to the line.
 *
 * Three layers, back to front: the field (field.ts, one WebGL2 pass at reduced resolution),
 * Wisp (the brand's own engine), and the words (DOM). No motion library. The frame loop runs
 * only while Ghost is forward, at 30 frames a second unless the voice or the arrival needs 60,
 * and stops dead at rest, so the line costs nothing.
 */
import type { PendingProposal, QuestionView, Turn } from "@ghost/client-ts";
import { agentName, splitSaid, useGhost, youSpeaking } from "@ghost/ui";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { postureOf } from "../app/posture";
import { useSaying, useSessionQuestions } from "../screens/session/live";
import { Mascot } from "../ui/Mascot";
import { createField, type Field, type FieldInputs } from "./field";
import { elapsedText, fpsOf, isForward, labelOf, msText, type Scene, sceneOf, stageX, telemetryOf } from "./scene";
import { hitRects, log, openApp, setRegion, summon, useShellWord } from "./shell";

const params = new URLSearchParams(window.location.search);
const reducedQuery = typeof matchMedia !== "undefined" ? matchMedia("(prefers-reduced-motion: reduce)") : null;
const reduced = () => params.get("still") === "1" || (reducedQuery?.matches ?? false);
/** Drawing pixels per CSS pixel for the field: 0.4 is a sixth of the pixels, and soft is the look. */
const RES = Number(params.get("res") ?? 0.4);
/** Measurement switches (development): `?field=0` leaves the field undrawn, `?wisp=0` leaves Wisp out. */
const DRAW_FIELD = params.get("field") !== "0";
const DRAW_WISP = params.get("wisp") !== "0";

const WAKE_MS = 950;
const DISMISS_MS = 750;

/** Each scene's light, RGB 0..1: the design system's phase hues, pushed to neon on black. */
const TINT: Record<Scene, [number, number, number]> = {
  hidden: [0.35, 0.85, 1.0],
  line: [0.35, 0.85, 1.0],
  hold: [0.35, 0.85, 1.0],
  you: [0.3, 1.0, 0.72],
  listening: [0.3, 1.0, 0.72],
  thinking: [0.66, 0.5, 1.0],
  speaking: [0.35, 0.82, 1.0],
  okay: [1.0, 0.7, 0.25],
  question: [0.66, 0.5, 1.0],
  ending: [0.35, 0.85, 1.0],
};

const MOOD: Record<Scene, string | null> = {
  hidden: null, line: null, hold: "hold", you: "listening", listening: "listening", thinking: "thinking",
  speaking: "speaking", okay: "listening", question: "listening", ending: "speaking",
};

const ease = (from: number, to: number, rate: number, dt: number) => from + (to - from) * (1 - Math.exp(-rate * dt));

function lastOf(turns: readonly Turn[], role: Turn["role"], session: string | null): Turn | null {
  for (let i = turns.length - 1; i >= 0; i--) if (turns[i].role === role && turns[i].session_id === session) return turns[i];
  return null;
}

export function FloatingRoom() {
  const { client, status, connected, turns, pending, activity, voice } = useGhost();
  const shell = useShellWord();
  const session = status?.session_id ?? null;
  const deskSession = connected && !!session && status?.session_origin !== "phone";
  const micOff = !!status?.not_listening;

  // When this session started here: the questions asked since then are its own.
  const [since, setSince] = useState<string | null>(null);
  const [startedAt, setStartedAt] = useState<string | null>(null);
  useEffect(() => {
    setSince(session ? new Date(Date.now() - 5_000).toISOString() : null);
    setStartedAt(session ? new Date().toISOString() : null);
  }, [session]);
  const questions = useSessionQuestions(since);
  const question: QuestionView | null = useMemo(() => [...questions].reverse().find((q) => q.state === "open") ?? null, [questions]);

  // The one posture rule (posture.ts): floating, the main window not the one in use.
  const posture = postureOf({
    hold: "auto",
    deskSession,
    idleMs: 0,
    awayAfterMs: null,
    sessionIdleMs: null,
    floating: shell.capture !== "yes",
    windowActive: shell.mainActive,
  }).posture;
  const ruled = sceneOf({
    posture,
    capture: shell.capture,
    phase: status?.phase ?? null,
    ending: !!status?.ending,
    youTalking: youSpeaking(activity),
    pending: !!pending,
    question: !!question,
  });

  // A full-screen app on the only monitor: stay out of the way (the voice goes on).
  const [covered, setCovered] = useState(false);
  const [side, setSide] = useState<"centre" | "left" | "right">("centre");
  const forwardWanted = isForward(ruled);
  const scene: Scene = forwardWanted && covered ? "line" : ruled;
  const forward = isForward(scene);

  // The arrival and the leaving, as states of their own so the scene can animate between.
  // Timed by the frame loop, not by timers: WebKit throttles a page's timers in a window that
  // never takes focus, and a leaving that outlived its animation kept the loop running.
  const [arriving, setArriving] = useState(false);
  const [leaving, setLeaving] = useState(false);
  const moved = useRef({ arrive: 0, leave: 0 });
  const wasForward = useRef(false);
  const [trigger, setTrigger] = useState(0);
  useEffect(() => {
    if (forwardWanted && !wasForward.current) {
      wasForward.current = true;
      void summon().then((s) => {
        setCovered(!!s.choice?.covered);
        setSide(s.choice?.side ?? "centre");
        log(`summon ${JSON.stringify(s)}`);
      });
      setLeaving(false);
      setArriving(!reduced());
      moved.current.arrive = performance.now();
      setTrigger((n) => n + 1);
      return;
    }
    if (!forwardWanted && wasForward.current) {
      wasForward.current = false;
      setCovered(false);
      setArriving(false);
      setLeaving(!reduced());
      moved.current.leave = performance.now();
    }
  }, [forwardWanted]);

  const shown = forward || leaving;
  // The line is the newest thing said: Ghost's reply, or the operator's words while Ghost works
  // on them (never while they speak: those appear once the turn closes, #73).
  const lastReply = lastOf(turns, "assistant", session);
  const heard = scene === "you" ? null : lastOf(turns, "user", session);
  const yoursIsNewer = !!heard && (!lastReply || heard.at > lastReply.at);
  const reply = yoursIsNewer ? null : lastReply;
  const saying = useSaying(turns, status?.phase === "speaking", activity, 1);
  const apps = status?.apps ?? [];
  const tele = useMemo(() => telemetryOf(activity, apps), [activity, apps.join(",")]);
  const usingApp = tele.rows.filter((r) => r.ms == null).map((r) => r.app ?? r.name)[0] ?? null;
  const agent = agentName(status?.conversation ?? null);
  const label = labelOf(scene, { agent, using: usingApp, asker: question?.asked_by ?? null, micOff });

  // ---------------------------------------------------------------- the frame loop
  const root = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const wisp = useRef<HTMLDivElement>(null);
  const clockEl = useRef<HTMLSpanElement>(null);
  const startedRef = useRef<string | null>(null);
  const field = useRef<Field | null>(null);
  const raf = useRef(0);
  const live = useRef({ scene, arriving, leaving, x: stageX(side), nodes: [] as { on: number }[] });
  live.current = { ...live.current, scene, arriving, leaving, x: stageX(side) };
  const anim = useRef({ dim: 0, glow: 0, ring: 0, okay: 0, you: 0, level: 0, tint: TINT.line, last: 0, since: 0, frames: 0, js: 0, from: 0, gaps: [] as number[] });
  // The apps reached this turn, lit while their calls run (the reach of the thinking).
  const reach = useMemo(() => {
    const names = tele.apps.slice(-6);
    return names.map((a) => ({ name: a, on: tele.rows.some((r) => r.app === a && r.ms == null) ? 1 : 0.28 }));
  }, [tele]);
  live.current.nodes = reach;

  const geometry = useCallback(() => {
    const w = window.innerWidth;
    const h = window.innerHeight;
    const core: [number, number] = [w * live.current.x, h * 0.3];
    const m = Math.min(w, h);
    const n = Math.max(1, live.current.nodes.length);
    const nodes = live.current.nodes.map((_, i) => {
      const a = Math.PI * (1.15 + (n === 1 ? 0.35 : (i / (n - 1)) * 0.7));
      return { x: core[0] + Math.cos(a) * m * 0.42, y: core[1] + 0.05 * m + Math.sin(a) * m * 0.22 };
    });
    return { w, h, core, nodes };
  }, []);

  const frame = useCallback(
    (now: number) => {
      const L = live.current;
      const A = anim.current;
      const t0 = performance.now();
      const fps = fpsOf(L.scene, L.arriving) || (L.leaving ? 60 : 0);
      // Skip a frame when the scene reads at 30 and the last was under 30 ms ago.
      if (A.last && fps === 30 && now - A.last < 30) {
        raf.current = requestAnimationFrame(frame);
        return;
      }
      const dt = A.last ? Math.min(0.1, (now - A.last) / 1000) : 1 / 60;
      if (A.last) A.gaps.push(now - A.last);
      A.last = now;
      const t = now / 1000;
      // The arrival and the leaving end on the frame clock.
      if (L.arriving && performance.now() - moved.current.arrive > WAKE_MS) {
        L.arriving = false;
        setArriving(false);
      }
      if (L.leaving && performance.now() - moved.current.leave > DISMISS_MS) {
        L.leaving = false;
        setLeaving(false);
      }
      const g = geometry();
      const on = isForward(L.scene);
      const target = {
        dim: !on ? 0 : L.scene === "speaking" || L.scene === "ending" ? 0.5 : 0.6,
        glow: !on ? 0 : L.arriving ? 1.3 : 1,
        ring: L.scene === "you" ? 1 : L.scene === "listening" ? 0.35 : 0,
        okay: L.scene === "okay" ? 1 : 0,
      };
      const fast = L.leaving ? 9 : 5;
      A.dim = ease(A.dim, target.dim, fast, dt);
      A.glow = ease(A.glow, target.glow, fast, dt);
      A.ring = ease(A.ring, target.ring, 6, dt);
      A.okay = ease(A.okay, target.okay, 5, dt);
      const tint = TINT[L.scene];
      A.tint = [ease(A.tint[0], tint[0], 4, dt), ease(A.tint[1], tint[1], 4, dt), ease(A.tint[2], tint[2], 4, dt)];
      // The operator speaking is a sign, the same at any volume: a slow breath, never the mic.
      const breath = L.scene === "you" ? 0.45 + 0.25 * Math.sin(t * 2.4) : 0;
      A.you = ease(A.you, breath, 8, dt);
      // Ghost's own voice level, as the daemon reports it at the speaker.
      const v = voice.get();
      const level = performance.now() - v.at < 400 ? v.level : 0;
      A.level = ease(A.level, level, 22, dt);
      // The session's clock, from the frame loop (the page's timers are throttled here).
      if (clockEl.current) {
        const e = elapsedText(startedRef.current, Date.now());
        if (e && clockEl.current.textContent !== ` · ${e}`) clockEl.current.textContent = ` · ${e}`;
      }
      if (wisp.current) wisp.current.style.setProperty("--flt-pulse", (1 + 0.035 * A.level + 0.02 * A.you).toFixed(4));
      const sinceArrive = performance.now() - moved.current.arrive;
      const shock = L.arriving ? (sinceArrive - WAKE_MS * 0.62) / 700 : -1;
      const scan = L.arriving ? Math.min(1.05, sinceArrive / (WAKE_MS * 0.9)) : -1;
      const fall = L.leaving ? Math.min(1, (performance.now() - moved.current.leave) / DISMISS_MS) : 0;
      const core: [number, number] = [g.core[0], g.core[1] + (g.h - g.core[1]) * fall * fall];
      const inputs: FieldInputs = {
        time: t, dim: A.dim, scan, core, glow: A.glow, tint: A.tint, voice: A.you, ring: A.ring, level: A.level, okay: A.okay,
        shock: shock >= 0 && shock <= 1 ? shock : -1,
        nodes: g.nodes.map((n, i) => ({ x: n.x, y: n.y, on: L.scene === "thinking" ? (L.nodes[i]?.on ?? 0) : 0 })),
      };
      if (DRAW_FIELD) field.current?.draw(inputs);
      A.frames++;
      A.js += performance.now() - t0;
      if (now - A.from > 5000) flush(now);
      if (!on && !L.leaving && A.dim < 0.004 && A.glow < 0.004) {
        rest();
        return;
      }
      if (reduced()) return; // one frame per change under reduced motion
      raf.current = requestAnimationFrame(frame);
    },
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [geometry, voice],
  );

  const flush = (now: number) => {
    const A = anim.current;
    if (A.frames > 2) {
      const gaps = [...A.gaps].sort((a, b) => a - b);
      const mean = gaps.reduce((a, b) => a + b, 0) / Math.max(1, gaps.length);
      log(`frames scene=${live.current.scene} fps=${(1000 / mean).toFixed(1)} p95_ms=${(gaps[Math.floor(gaps.length * 0.95)] ?? 0).toFixed(1)} js_ms=${(A.js / A.frames).toFixed(2)} dim=${A.dim.toFixed(4)} glow=${A.glow.toFixed(4)} leaving=${live.current.leaving}`);
    }
    A.frames = 0;
    A.js = 0;
    A.gaps = [];
    A.from = now;
  };

  const rest = () => {
    cancelAnimationFrame(raf.current);
    raf.current = 0;
    flush(performance.now());
    field.current?.clear();
    if (canvas.current) canvas.current.style.visibility = "hidden";
    anim.current.last = 0;
    log(`rest: loop stopped`);
  };

  // Start the loop when Ghost comes forward (or draws one frame per change, reduced).
  useEffect(() => {
    if (!shown) return;
    if (canvas.current && field.current?.ok) canvas.current.style.visibility = "visible";
    if (raf.current && !reduced()) return;
    cancelAnimationFrame(raf.current);
    anim.current.last = 0;
    anim.current.from = performance.now();
    raf.current = requestAnimationFrame(frame);
    log(`loop start scene=${scene} arriving=${arriving} leaving=${leaving}`);
  }, [shown, scene, arriving, leaving, frame]);

  useEffect(() => {
    if (!canvas.current) return;
    try {
      field.current = createField(canvas.current, RES);
    } catch (e) {
      log(`field failed: ${String(e)}`);
    }
    canvas.current.style.visibility = "hidden";
    log(`ready webgl2=${field.current?.ok ?? false} dpr=${window.devicePixelRatio} viewport=${window.innerWidth}x${window.innerHeight} res=${RES} reduced=${reduced()}`);
    const onResize = () => field.current?.resize();
    window.addEventListener("resize", onResize);
    return () => {
      window.removeEventListener("resize", onResize);
      cancelAnimationFrame(raf.current);
    };
  }, []);

  // The zoom from depth, and back. WAAPI: the compositor runs it.
  useLayoutEffect(() => {
    const el = wisp.current;
    if (!el) return;
    const quick = reduced();
    if (arriving) {
      el.animate(
        [
          { transform: "perspective(900px) translate3d(0, 40px, -2600px)", filter: "blur(18px) brightness(3)", opacity: 0 },
          { transform: "perspective(900px) translate3d(0, -6px, 40px)", filter: "blur(0px) brightness(1.6)", opacity: 1, offset: 0.78 },
          { transform: "perspective(900px) translate3d(0, 0, 0)", filter: "blur(0px) brightness(1)", opacity: 1 },
        ],
        // "backwards", not "both": a filter held after the animation is a filter on every frame.
        { duration: quick ? 0 : WAKE_MS, easing: "cubic-bezier(.16,.84,.24,1)", fill: "backwards" },
      );
    } else if (leaving) {
      el.animate(
        [
          { transform: "perspective(900px) translate3d(0, 0, 0)", filter: "blur(0px)", opacity: 1 },
          { transform: "perspective(900px) translate3d(0, 30vh, -1800px)", filter: "blur(10px)", opacity: 0 },
        ],
        { duration: quick ? 0 : DISMISS_MS, easing: "cubic-bezier(.5,0,.75,0)", fill: "both" },
      );
    }
  }, [arriving, leaving]);

  // The input region follows what can be clicked: nothing at rest, the buttons otherwise.
  useEffect(() => {
    if (!root.current) return;
    const send = () => setRegion(forward ? hitRects(root.current!) : []);
    send();
    const id = window.setTimeout(send, 450); // after the arrival settles
    return () => window.clearTimeout(id);
  }, [forward, scene, pending?.id, question?.key, reply?.at]);

  // ---------------------------------------------------------------- answers
  const [busy, setBusy] = useState(false);
  const decide = async (p: PendingProposal, yes: boolean) => {
    setBusy(true);
    try {
      await (yes ? client.confirmProposal(p.id) : client.cancelProposal(p.id));
    } catch (e) {
      log(`decide failed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };
  const answer = async (q: QuestionView, choice: string) => {
    setBusy(true);
    try {
      await client.answerQuestion(q.key, choice);
    } catch (e) {
      log(`answer failed: ${String(e)}`);
    } finally {
      setBusy(false);
    }
  };

  // ---------------------------------------------------------------- what is drawn
  const g = geometry();
  const said = reply ? saying[reply.at] : undefined;
  const parts = reply ? splitParts(reply.text, said, status?.phase === "speaking") : null;
  startedRef.current = startedAt;
  const okayCard = scene === "okay" && pending;
  const questionCard = scene === "question" && question;

  return (
    <div
      ref={root}
      className="flt-root"
      data-scene={scene}
      data-shown={shown ? "1" : "0"}
      data-mic={micOff ? "off" : "on"}
      style={{ ["--flt-x" as string]: `${stageX(side) * 100}%` }}
    >
      <canvas ref={canvas} className="flt-field" aria-hidden />

      {/* One sentence a screen reader hears per change; the type itself is decoration. */}
      <p className="flt-sr" role="status" aria-live="polite">
        {scene === "hidden" ? "" : forward ? [label, scene === "speaking" && reply ? reply.text : ""].filter(Boolean).join(". ") : micOff ? "Ghost is not listening." : ""}
      </p>

      {/* At rest: the line says Ghost hears for its name; struck and red when it can't (ADR 0057). */}
      {scene !== "hidden" && (
        <div className="flt-line" aria-hidden data-hold={scene === "hold" ? "1" : undefined}>
          {micOff ? <MicOff /> : <span className="flt-line__light" />}
          {scene === "hold" && <span className="flt-line__hold">{label}</span>}
        </div>
      )}

      {shown && (
        <div className="flt-stage" style={{ left: g.core[0], top: g.core[1] }}>
          <div ref={wisp} className="flt-wisp" aria-hidden>
            {DRAW_WISP && <Mascot size={176} mood={leaving ? null : MOOD[scene]} trigger={trigger} level={voice} idle maxFps={fpsOf(scene, arriving) || 60} />}
          </div>
          <p className="flt-label">
            <span className="flt-label__dot" />
            {label}
            {micOff && scene !== "listening" ? <span className="flt-label__mic"> · mic off</span> : null}
          </p>
        </div>
      )}

      {shown &&
        reach.map((r, i) => (
          <div key={r.name} className="flt-node" data-on={r.on > 0.6 ? "1" : "0"} hidden={scene !== "thinking"} style={{ left: g.nodes[i]?.x, top: g.nodes[i]?.y }}>
            <span className="flt-node__name">{r.name.charAt(0).toUpperCase() + r.name.slice(1)}</span>
          </div>
        ))}

      {forward && (
        <div className="flt-words" aria-hidden={!okayCard && !questionCard ? true : undefined}>
          {heard && !okayCard && !questionCard && <p className={yoursIsNewer ? "flt-heard flt-heard--line" : "flt-heard"}>{heard.text}</p>}
          {okayCard ? (
            <div className="flt-ask flt-ask--okay" role="alertdialog" aria-label={`Waiting for your yes: ${pending.summary}`}>
              <p className="flt-ask__over">
                {agentName(status?.conversation ?? null)} asks · <span className="flt-mono">{pending.operation}</span>
              </p>
              <p className="flt-ask__q">{pending.summary}?</p>
              <div className="flt-ask__acts">
                <button type="button" data-hit className="flt-word flt-word--yes" disabled={busy} onClick={() => void decide(pending, true)}>
                  Yes
                </button>
                <button type="button" data-hit className="flt-word" disabled={busy} onClick={() => void decide(pending, false)}>
                  No
                </button>
                <span className="flt-ask__or">or say it</span>
              </div>
            </div>
          ) : questionCard ? (
            <div className="flt-ask" role="alertdialog" aria-label={`${question.asked_by} asks: ${question.question}`}>
              <p className="flt-ask__over">{question.asked_by} asks</p>
              <p className="flt-ask__q">{question.question}</p>
              <div className="flt-ask__acts">
                {question.choices.map((c) => (
                  <button key={c} type="button" data-hit className="flt-word" disabled={busy} onClick={() => void answer(question, c)}>
                    {c}
                  </button>
                ))}
                <span className="flt-ask__or">{question.choices.length ? "or say it" : "Answer by voice, or in Ghost"}</span>
              </div>
            </div>
          ) : (
            parts && (
              <p className="flt-reply">
                <span className="flt-reply__said">{parts.said}</span>
                <span className="flt-reply__now">{parts.now}</span>
                <span className="flt-reply__rest">{parts.rest}</span>
              </p>
            )
          )}
        </div>
      )}

      {forward && (
        <aside className="flt-margin" aria-hidden>
          <p className="flt-margin__head">
            {agent.toUpperCase()}
            <span ref={clockEl} />
          </p>
          <p className="flt-margin__counts">
            <span>calls {tele.calls}</span>
            <span>open {tele.open}</span>
            <span>apps {tele.apps.length}</span>
            {status?.waiting ? <span>waiting {status.waiting}</span> : null}
          </p>
          <ol className="flt-margin__calls">
            {tele.rows.map((r) => (
              <li key={r.id} data-open={r.ms == null ? "1" : undefined}>
                <span>{r.at}</span>
                <span className="flt-margin__name">{r.name}</span>
                <span>{msText(r.ms)}</span>
              </li>
            ))}
          </ol>
          <button type="button" data-hit className="flt-open" onClick={openApp}>
            Open in Ghost
          </button>
        </aside>
      )}
    </div>
  );
}

/** The reply split for the type: said, the clause being said now, and the rest. */
function splitParts(text: string, s: { chars: number; now?: { end: number } | null; cut: boolean } | undefined, speaking: boolean) {
  if (!s) return speaking ? { said: "", now: "", rest: text } : { said: text, now: "", rest: "" };
  const { said, rest } = splitSaid(text, s.chars);
  if (s.now && !s.cut) {
    const end = Math.max(s.chars, s.now.end);
    return { said, now: text.slice(s.chars, end), rest: text.slice(end) };
  }
  return { said, now: "", rest };
}


function MicOff() {
  return (
    <svg className="flt-line__mic" viewBox="0 0 24 24" width="16" height="16" aria-hidden>
      <path d="M9 9v3a3 3 0 0 0 5.1 2.1M15 9.3V5a3 3 0 0 0-5.9-.7M19 10v2a7 7 0 0 1-.6 2.8M5 10v2a7 7 0 0 0 11.6 5.3M12 19v3M2 2l20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}
