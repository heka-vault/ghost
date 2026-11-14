/**
 * The floating room (ADR 0059; docs/design-system.md, The floating room): the room posture
 * without a window. At rest a thin line of light at the bottom edge of a monitor (a struck red
 * microphone when Ghost isn't listening, ADR 0057); when a spoken session at this desk floats
 * (`postureOf`), Ghost comes forward from depth over the desktop, beside the work by default.
 *
 * It is a voice, so it shows no transcript (owner, 2026-11-12): neither the operator's words nor
 * Ghost's reply. What it shows is the state (Wisp, the light and its colour, a few words under
 * Wisp), where Ghost is looking (reach.ts: a named place and a beam for every app and agent the
 * turn reaches, lit while its call runs, the answer coming back in proportion to what it brought,
 * a quiet trace after), and only what sound can't carry: a change waiting for a yes and an
 * agent's question, with their buttons. Captions are a setting, off by default.
 *
 * Three layers, back to front: the field (field.ts, one WebGL2 pass at reduced resolution),
 * Wisp (the brand's own engine), and the few words and buttons (DOM). Where each sits is
 * layout.ts, a pure function of the monitor and the operator's placement. No motion library.
 * The frame loop runs only while Ghost is forward, at 30 frames a second (60 for the arrival),
 * and stops dead at rest, so the line costs nothing.
 *
 * While the screen is shared it does what the operator chose (the shell's `share.rs`): nothing
 * at all (the default), a small card asking Show or Hide, or Ghost in the share with a mark
 * that says the screen is being shared.
 *
 * `stage`: the same room in an ordinary window on its own backdrop (`stage.html`, ADR 0059 as
 * amended 2026-11-13), for showing Ghost to people. Opened on purpose and meant to be seen: it
 * never hides from a share, Ghost waits in the middle of it rather than as a line, a session
 * comes forward whichever window has the keyboard, there are no buttons but a yes's and a
 * question's, and Ghost's own words are captioned, since the people watching may not hear it.
 */
import { typedSession } from "../app/listening";
import type { PendingProposal, QuestionView, Turn } from "@ghost/client-ts";
import { agentName, splitSaid, useGhost, youSpeaking } from "@ghost/ui";
import { useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";

import { postureOf } from "../app/posture";
import { useSaying, useSessionQuestions } from "../screens/session/live";
import { Mascot } from "../ui/Mascot";
import { createField, type Field, type FieldInputs } from "./field";
import { layoutOf, type Layout } from "./layout";
import { litOf, namesText, type Place, reachOf, returnOf, sizeOf, thisTurn } from "./reach";
import { captionsShown, captionText, fpsOf, goneAfterGoodbye, isForward, labelOf, type Scene, sceneOf } from "./scene";
import { answerShare, hitRects, log, openApp, sendAway, setRegion, sound as readSound, type Sound, summon, useShellWord } from "./shell";

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

/** A scene's light as CSS, for Wisp's lit edge (the mark's light, `Mascot`'s `lit`). */
const tintCss = (s: Scene) => `rgb(${TINT[s].map((v) => Math.round(v * 255)).join(" ")})`;

const MOOD: Record<Scene, string | null> = {
  hidden: null, line: null, hold: "hold", you: "listening", listening: "listening", thinking: "thinking",
  speaking: "speaking", okay: "listening", question: "listening", ending: "speaking",
};

const ease = (from: number, to: number, rate: number, dt: number) => from + (to - from) * (1 - Math.exp(-rate * dt));

function lastOf(turns: readonly Turn[], role: Turn["role"], session: string | null): Turn | null {
  for (let i = turns.length - 1; i >= 0; i--) if (turns[i].role === role && turns[i].session_id === session) return turns[i];
  return null;
}

/** The window's size, kept in state so the layout follows a move to another monitor. */
function useViewport(): { w: number; h: number } {
  const [v, setV] = useState({ w: window.innerWidth, h: window.innerHeight });
  useEffect(() => {
    const on = () => setV({ w: window.innerWidth, h: window.innerHeight });
    window.addEventListener("resize", on);
    return () => window.removeEventListener("resize", on);
  }, []);
  return v;
}

export function FloatingRoom({ stage = false }: { stage?: boolean } = {}) {
  const { client, status, connected, turns, pending, activity, voice } = useGhost();
  const word = useShellWord();
  // The stage is its own window, meant to be seen: no share hides it, Ghost in the middle,
  // Ghost's words captioned for the people watching.
  const shell = stage ? { ...word, mainActive: false, capture: null, sharing: "none" as const, placement: "centre" as const, captions: "always" as const } : word;
  const view = useViewport();
  const session = status?.session_id ?? null;
  const deskSession = connected && !!session && status?.session_origin !== "phone";
  const micOff = !!status?.not_listening;
  // A typed session opened no microphone (2026-11-13): Wisp and the label say typing.
  const typed = typedSession(status);

  // When this session started here: the questions asked since then are its own.
  const [since, setSince] = useState<string | null>(null);
  useEffect(() => {
    setSince(session ? new Date(Date.now() - 5_000).toISOString() : null);
  }, [session]);
  const questions = useSessionQuestions(since);
  const question: QuestionView | null = useMemo(() => [...questions].reverse().find((q) => q.state === "open") ?? null, [questions]);

  // Where Ghost is looking this turn (reach.ts), from the activity alone.
  const apps = status?.apps ?? [];
  const reach = useMemo(() => reachOf(activity, apps), [activity, apps.join(",")]);
  const openCalls = reach.places.reduce((n, p) => n + p.open, 0);

  // The goodbye must not linger: gone once its voice has been quiet a moment, or after 4 s.
  const ending = !!status?.ending;
  const [gone, setGone] = useState(false);
  const goodbye = useRef<{ since: number | null; heard: boolean; quietSince: number }>({ since: null, heard: false, quietSince: 0 });
  useEffect(() => {
    goodbye.current = { since: ending ? performance.now() : null, heard: false, quietSince: performance.now() };
    setGone(false);
  }, [ending, session]);

  // The one posture rule (posture.ts): floating, the main window not the one in use.
  const posture = postureOf({
    hold: "auto",
    deskSession,
    idleMs: 0,
    awayAfterMs: null,
    sessionIdleMs: null,
    floating: shell.sharing === "none" || shell.sharing === "shown",
    windowActive: shell.mainActive,
  }).posture;
  const ruled = sceneOf({
    posture,
    capture: shell.capture,
    sharing: shell.sharing,
    phase: status?.phase ?? null,
    ending,
    youTalking: youSpeaking(activity),
    pending: !!pending,
    question: !!question,
    openCalls,
    gone,
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
  const [snd, setSnd] = useState<Sound | null>(null);
  useEffect(() => {
    if (forwardWanted && !wasForward.current) {
      wasForward.current = true;
      // The stage is its own window: nothing to move, Ghost in its middle.
      if (!stage) {
        void summon().then((s) => {
          setCovered(!!s.choice?.covered);
          setSide(s.choice?.side ?? "centre");
          log(`summon ${JSON.stringify(s)}`);
        });
      }
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
  const reply = lastOf(turns, "assistant", session);
  const saying = useSaying(turns, status?.phase === "speaking", activity, 1);
  const agent = agentName(status?.conversation ?? null);

  // Captions: read the output's mute when Ghost starts to speak (only if the setting needs it).
  useEffect(() => {
    if (shell.captions !== "muted" || (scene !== "speaking" && scene !== "ending")) return;
    void readSound().then(setSnd);
  }, [shell.captions, scene, reply?.at]);
  const captions = captionsShown(shell.captions, scene, snd);

  // The model as the wait: seconds since the operator's turn closed, with nothing out and no reply.
  const [modelSecs, setModelSecs] = useState<number | null>(null);
  const turnClosedAt = useMemo(() => {
    for (let i = activity.length - 1; i >= 0; i--) if (activity[i].kind === "you_stopped") return new Date(activity[i].at).getTime();
    return null;
  }, [activity]);
  const waitingOnModel = scene === "thinking" && openCalls === 0 && turnClosedAt != null && !(reply && new Date(reply.at).getTime() > turnClosedAt) && thisTurn(activity).every((a) => a.kind !== "call");

  // ---------------------------------------------------------------- what came back, by size
  // `GET /calls/{id}` (ADR 0058): its length only, never shown, for the light coming back.
  const sizes = useRef<Record<string, number>>({});
  const [, bumpSizes] = useState(0);
  useEffect(() => {
    for (const p of reach.places) {
      if (p.open > 0) continue;
      for (const id of p.ids) {
        if (id in sizes.current) continue;
        sizes.current[id] = 0;
        client
          .call(id)
          .then((c) => {
            sizes.current[id] = c.preview.length + Number(c.dropped ?? 0);
            bumpSizes((n) => n + 1);
          })
          .catch(() => undefined);
      }
    }
  }, [reach, client]);
  const placeSize = (p: Place) => sizeOf(Math.max(0, ...p.ids.map((id) => sizes.current[id] ?? 0)));

  const card = (scene === "okay" && !!pending) || (scene === "question" && !!question);
  const reaching = scene === "thinking" || scene === "speaking" || scene === "you" || scene === "listening";
  const places = reaching ? reach.places : [];
  const layout: Layout = useMemo(
    () => layoutOf({ w: view.w, h: view.h, placement: shell.placement, side: side === "centre" ? null : side, places: places.length, card, captions }),
    // `place.rs` says which side Ghost's stage sits on: away from the work; right when not known.
    // eslint-disable-next-line react-hooks/exhaustive-deps
    [view.w, view.h, shell.placement, side, places.length, card, captions],
  );
  const using = namesText(reach.places.filter((p) => p.open > 0).map((p) => p.name));
  const label = labelOf(scene, { agent, using, asker: question?.asked_by ?? null, micOff, modelSecs: waitingOnModel ? modelSecs : null, typed });

  // ---------------------------------------------------------------- the frame loop
  const root = useRef<HTMLDivElement>(null);
  const canvas = useRef<HTMLCanvasElement>(null);
  const wisp = useRef<HTMLDivElement>(null);
  const nodeEls = useRef<(HTMLDivElement | null)[]>([]);
  const field = useRef<Field | null>(null);
  const raf = useRef(0);
  const live = useRef({ scene, arriving, leaving, layout, places, waitingOnModel, turnClosedAt, modelSecs: null as number | null });
  live.current = { ...live.current, scene, arriving, leaving, layout, places, waitingOnModel, turnClosedAt };
  const anim = useRef({ dim: 0, glow: 0, ring: 0, okay: 0, you: 0, level: 0, tint: TINT.line, last: 0, since: 0, frames: 0, js: 0, from: 0, gaps: [] as number[] });

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
      const g = L.layout;
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
      const fresh = performance.now() - v.at < 400;
      const level = fresh ? v.level : 0;
      A.level = ease(A.level, level, 22, dt);
      // The goodbye: leave once its voice has been quiet a moment, or after its time.
      const gb = goodbye.current;
      if (gb.since != null) {
        if (level > 0.02) {
          gb.heard = true;
          gb.quietSince = now;
        }
        if (goneAfterGoodbye({ endingFor: now - gb.since, quietFor: now - gb.quietSince, heard: gb.heard })) {
          gb.since = null;
          setGone(true);
        }
      }
      // The model as the wait, in whole seconds (a state change at most once a second).
      const ms = L.waitingOnModel && L.turnClosedAt != null ? Math.floor((Date.now() - L.turnClosedAt) / 1000) : null;
      if (ms !== L.modelSecs) {
        L.modelSecs = ms;
        setModelSecs(ms);
      }
      if (wisp.current) wisp.current.style.setProperty("--flt-pulse", (1 + 0.035 * A.level + 0.02 * A.you).toFixed(4));
      const sinceArrive = performance.now() - moved.current.arrive;
      const shock = L.arriving ? (sinceArrive - WAKE_MS * 0.62) / 700 : -1;
      const scan = L.arriving ? Math.min(1.05, sinceArrive / (WAKE_MS * 0.9)) : -1;
      const fall = L.leaving ? Math.min(1, (performance.now() - moved.current.leave) / DISMISS_MS) : 0;
      const core: [number, number] = [g.core[0], g.core[1] + (window.innerHeight - g.core[1]) * fall * fall];
      // Where Ghost is looking: each place's light, the packets out, the answer coming back.
      const wall = Date.now();
      const nodes = L.places.map((p, i) => {
        const lit = L.leaving ? 0 : litOf(p, wall);
        const el = nodeEls.current[i];
        if (el) {
          const want = lit > 0.6 ? "1" : "0";
          if (el.dataset.on !== want) el.dataset.on = want;
        }
        const at = g.places[i] ?? g.core;
        return { x: at[0], y: at[1], on: lit, out: p.open > 0, back: returnOf(p, wall), size: placeSize(p) };
      });
      const inputs: FieldInputs = {
        time: t, dim: A.dim, scan, core, glow: A.glow, tint: A.tint, voice: A.you, ring: A.ring, level: A.level, okay: A.okay,
        shock: shock >= 0 && shock <= 1 ? shock : -1,
        nodes,
        size: g.size,
        region: L.scene === "line" || L.leaving ? null : [g.bounds.x, g.bounds.y, g.bounds.w, g.bounds.h],
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
    [voice],
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
  }, [shown, scene, arriving, leaving, frame, places.length, openCalls]);

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

  // The input region follows what can be clicked: nothing at rest, the buttons otherwise
  // (the share card's too). The stage is an ordinary window and takes its own clicks.
  const asking = !stage && shell.sharing === "asking";
  useEffect(() => {
    if (!root.current || stage) return;
    const send = () => setRegion(forward || asking ? hitRects(root.current!) : []);
    send();
    const id = window.setTimeout(send, 450); // after the arrival settles
    return () => window.clearTimeout(id);
  }, [forward, asking, scene, pending?.id, question?.key, layout]);

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
  const said = reply ? saying[reply.at] : undefined;
  const caption = captions && reply ? captionText(reply.text, said ? Math.max(said.chars, said.now?.end ?? 0) : null) : "";
  const s = layout.scale;
  // Ghost is in a share by the operator's choice: say so, on the surface itself.
  const sharedMark = !stage && shell.sharing === "shown" && scene !== "hidden";

  return (
    <div
      ref={root}
      className="flt-root"
      data-scene={scene}
      data-shown={shown ? "1" : "0"}
      data-mic={micOff ? "off" : "on"}
      data-placement={shell.placement}
      data-stage={stage ? "1" : undefined}
      style={{ ["--flt-s" as string]: s.toFixed(3), ["--flt-wisp" as string]: `${layout.wisp}px` }}
    >
      <canvas ref={canvas} className="flt-field" aria-hidden />

      {/* One sentence a screen reader hears per change: the state, and Ghost's reply whole when it starts. */}
      <p className="flt-sr" role="status" aria-live="polite">
        {scene === "hidden" ? "" : forward ? [label, scene === "speaking" && reply ? splitSaid(reply.text, reply.text.length).said : ""].filter(Boolean).join(". ") : micOff ? "Ghost is not listening." : ""}
      </p>

      {/* At rest: the line says Ghost hears for its name; struck and red when it can't (ADR 0057). */}
      {scene !== "hidden" && (
        <div className="flt-line" aria-hidden data-hold={scene === "hold" ? "1" : undefined}>
          {micOff ? <MicOff /> : <span className="flt-line__light" />}
          {scene === "hold" && <span className="flt-line__hold">{label}</span>}
          {sharedMark && !forward && <SharedMark />}
        </div>
      )}

      {/* On stage, at rest: Ghost waits in the middle for its name, not as a line. */}
      {stage && !shown && (
        <div className="flt-rest" aria-hidden style={{ left: layout.core[0], top: layout.core[1] }}>
          <div className="flt-rest__wisp">{DRAW_WISP && <Mascot size={Math.round(layout.wisp * 0.8)} mood={scene === "hold" ? "hold" : null} idle maxFps={30} lit={tintCss("hold")} />}</div>
          <p className="flt-rest__say">{micOff ? "Mic off" : scene === "hold" ? "On hold · say “Ghost” to pick up" : "Say “Ghost”"}</p>
        </div>
      )}
      {stage && (
        <div className="flt-brand" aria-hidden>
          <span className="flt-brand__dot" data-live={connected ? "1" : "0"} />
          Ghost
        </div>
      )}

      {/* A share started and the operator chose to be asked: two buttons, nothing of Ghost. */}
      {asking && (
        <div className="flt-sharecard" role="alertdialog" aria-label="Your screen is being shared. Show Ghost in it?">
          <p className="flt-sharecard__q">
            <span className="flt-shared__dot" />
            Your screen is being shared. Show Ghost in it?
          </p>
          <div className="flt-ask__acts">
            <button type="button" data-hit className="flt-word flt-word--yes" onClick={() => answerShare(true)}>
              Show
            </button>
            <button type="button" data-hit className="flt-word" onClick={() => answerShare(false)}>
              Hide
            </button>
          </div>
        </div>
      )}

      {shown && (
        <div className="flt-stage" style={{ left: layout.core[0], top: layout.core[1] }}>
          <div ref={wisp} className="flt-wisp" aria-hidden>
            {DRAW_WISP && <Mascot size={layout.wisp} mood={leaving ? null : scene === "listening" && typed ? "passive" : MOOD[scene]} trigger={trigger} level={voice} idle maxFps={fpsOf(scene, arriving) || 60} lit={tintCss(scene)} />}
          </div>
          <div className="flt-under">
            {sharedMark && <SharedMark />}
            <p className="flt-label">
              <span className="flt-label__dot" />
              {label}
              {micOff && scene !== "listening" ? <span className="flt-label__mic"> · mic off</span> : null}
            </p>
            {forward && !stage && (
              <p className="flt-acts">
                <button type="button" data-hit className="flt-pill" onClick={openApp}>
                  Open in Ghost
                </button>
                <button type="button" data-hit className="flt-pill" title="Ends the session, as “Ghost, that’s all” does" onClick={() => sendAway(() => client.sleep())}>
                  Done
                </button>
              </p>
            )}
          </div>
        </div>
      )}

      {/* Where Ghost is looking: each place's name over its beam's end. */}
      {shown &&
        places.map((p, i) => (
          <div
            key={p.key}
            ref={(el) => {
              nodeEls.current[i] = el;
            }}
            className="flt-node"
            data-on="1"
            aria-hidden
            style={{ left: layout.places[i]?.[0], top: layout.places[i]?.[1] }}
          >
            <span className="flt-node__name">{p.name}</span>
            {p.word && <span className="flt-node__word">{p.word}</span>}
          </div>
        ))}
      {shown && reach.more > 0 && reaching && (
        <p className="flt-more" aria-hidden style={{ left: layout.core[0], top: layout.core[1] - layout.ring - 8 * s }}>
          +{reach.more} more
        </p>
      )}

      {forward && card && layout.card && (
        <div className="flt-card" style={{ left: layout.card.x, top: layout.card.y, width: layout.card.w, maxHeight: layout.card.h }}>
          {scene === "okay" && pending ? (
            <div className="flt-ask flt-ask--okay" role="alertdialog" aria-label={`Waiting for your yes: ${pending.summary}`}>
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
          ) : question ? (
            <div className="flt-ask" role="alertdialog" aria-label={`${question.asked_by} asks: ${question.question}`}>
              <p className="flt-ask__q">{question.question}</p>
              <div className="flt-ask__acts">
                {question.choices.slice(0, 4).map((c) => (
                  <button key={c} type="button" data-hit className="flt-word" disabled={busy} onClick={() => void answer(question, c)}>
                    {c}
                  </button>
                ))}
                <span className="flt-ask__or">{question.choices.length ? "or say it" : "Answer by voice, or in Ghost"}</span>
              </div>
            </div>
          ) : null}
        </div>
      )}

      {forward && caption && layout.captions && (
        <p className="flt-captions" aria-hidden style={{ left: layout.captions.x, top: layout.captions.y, width: layout.captions.w }}>
          {caption}
        </p>
      )}
    </div>
  );
}

/** The mark Ghost wears while it is in a share by the operator's choice. */
function SharedMark() {
  return (
    <span className="flt-shared" role="note">
      <span className="flt-shared__dot" />
      Screen shared
    </span>
  );
}

function MicOff() {
  return (
    <svg className="flt-line__mic" viewBox="0 0 24 24" width="16" height="16" aria-hidden>
      <path d="M9 9v3a3 3 0 0 0 5.1 2.1M15 9.3V5a3 3 0 0 0-5.9-.7M19 10v2a7 7 0 0 1-.6 2.8M5 10v2a7 7 0 0 0 11.6 5.3M12 19v3M2 2l20 20" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" />
    </svg>
  );
}
