/**
 * Wisp, Ghost's mascot (packages/ui/src/wisp.ts), drawn and driven with requestAnimationFrame:
 * the frame loop mutates the SVG's attributes through refs, so a playing mascot never
 * re-renders React (the rAF engine of the animated-svg-icons skill). React renders the rest pose once; the loop runs only while something moves and stops
 * when the character settles.
 *
 * `hovered` comes from the parent (a card, a button), as in the skill: Stage 1 on hover, Stage 2
 * on leave. Bump `trigger` to play it once the way a tap would on the phone. Reduced motion
 * shows the attentive face on hover without moving to it, and never loops.
 */
import { createElement, type ReactNode, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";

import { createMotion, detailFor, type MascotAttrs as Attrs, type MascotEl as El, type MascotFrame, stillPose, type VoiceLevel, WISP } from "@ghost/ui";

export interface MascotProps {
  /** Pixel size of the square. Under 24 draws the larger eyes; 32 and up adds the mitts. */
  size?: number;
  hovered?: boolean;
  /** Increment to play once (a tap, a click, Ghost calling for attention). */
  trigger?: number;
  /** A slow breathing, blinking loop at rest. Off by default: loops are for hero placements only. */
  idle?: boolean;
  /** What Wisp is doing between gestures: a voice phase (listening, thinking, speaking, hold, starting, offline). */
  mood?: string | null;
  /** Force reduced motion on or off; follows prefers-reduced-motion when omitted. */
  still?: boolean;
  /** Draw this exact pose and never animate (frame strips, tests). */
  frame?: MascotFrame;
  /** The wall clock for `frame`, for loops driven by time. */
  clock?: number;
  /** Accessible name; the mascot is decorative (aria-hidden) without one. */
  title?: string;
  className?: string;
  /**
   * Ghost's own voice level (`useGhost().voice`): in the speaking mood the mouth opens and the
   * body lifts with what is heard, instead of the stand-in rhythm. Never the microphone.
   */
  level?: VoiceLevel;
  /**
   * Draw at most this many frames a second (the floating room, ADR 0059, where every frame is
   * copied through XWayland). Unset: every display frame.
   */
  maxFps?: number;
  /**
   * Lit from within, as the brand's mark (`@ghost/ui` mark.ts): the body and mitts a white-hot
   * core cooling to this colour at the edge, instead of one flat colour. Ghost's own look only
   * (the floating room, the room view, Presence, the rail's chip); an agent's Wisp stays flat.
   */
  lit?: string;
}

/** A level older than this is stale: the voice stopped before its last 0 arrived. */
const LEVEL_STALE_MS = 250;

function usePrefersReducedMotion(): boolean {
  const [reduced, setReduced] = useState(() => typeof matchMedia !== "undefined" && matchMedia("(prefers-reduced-motion: reduce)").matches);
  useEffect(() => {
    const q = matchMedia("(prefers-reduced-motion: reduce)");
    const on = () => setReduced(q.matches);
    q.addEventListener("change", on);
    return () => q.removeEventListener("change", on);
  }, []);
  return reduced;
}

function draw(el: El, attrs: Attrs, key: number, light?: string): ReactNode {
  const live = el.part ? attrs[el.part] : undefined;
  const lit = light && el.props?.["data-flesh"] ? { fill: light } : undefined;
  return createElement(el.tag, { key, "data-part": el.part, ...el.props, ...live, ...lit }, el.children?.map((c, i) => draw(c, attrs, i)));
}

export function Mascot({ size = 96, hovered = false, trigger = 0, idle = false, mood = null, still, frame, clock = 0, title, className, level, maxFps, lit }: MascotProps) {
  const detail = detailFor(size);
  const { simple, props } = detail;
  const d = useMemo(() => ({ simple, props }), [simple, props]);
  const scene = useMemo(() => WISP.scene(d), [d]);
  // useId's output has characters a url(#...) reference can't carry.
  const maskId = `mascot-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const reduced = usePrefersReducedMotion();
  const isStill = frame !== undefined || (still ?? reduced);

  const svgRef = useRef<SVGSVGElement>(null);
  const parts = useRef(new Map<string, Element[]>());
  const written = useRef(new Map<string, string>());
  const motion = useMemo(() => createMotion(WISP), []);
  const raf = useRef(0);
  const live = useRef({ d, idle, isStill, level, mood, maxFps });
  live.current = { d, idle, isStill, level, mood, maxFps };
  const drawn = useRef(0);
  /** The voice's level, eased so the mouth moves like a mouth and not like a meter. */
  const mouth = useRef(0);

  // React only ever renders the rest pose (or a fixed `frame`). Its props never change while the
  // loop runs, so React never writes over the loop's attributes.
  // A partial frame is filled from the rest pose, as the motion machine fills its own.
  const attrs = WISP.attrs(frame ? { ...WISP.rest, ...frame } : WISP.rest, d, clock);

  const apply = (a: Attrs) => {
    for (const part in a) {
      const els = parts.current.get(part);
      if (!els) continue;
      for (const k in a[part]) {
        const value = String(a[part][k]);
        const key = `${part}.${k}`;
        if (written.current.get(key) === value) continue;
        written.current.set(key, value);
        for (const el of els) el.setAttribute(k, value);
      }
    }
  };

  useLayoutEffect(() => {
    const map = new Map<string, Element[]>();
    svgRef.current?.querySelectorAll("[data-part]").forEach((el) => {
      const p = el.getAttribute("data-part")!;
      map.set(p, [...(map.get(p) ?? []), el]);
    });
    parts.current = map;
    written.current.clear();
  }, [scene]);

  const run = () => {
    if (raf.current || live.current.isStill) return;
    const step = (now: number) => {
      raf.current = 0;
      const { d, idle, isStill, level, mood, maxFps } = live.current;
      if (isStill) return;
      // Under a cap, skip this display frame when the last drawing is recent enough.
      if (maxFps && now - drawn.current < 1000 / maxFps - 4) {
        raf.current = requestAnimationFrame(step);
        return;
      }
      drawn.current = now;
      let f = motion.frame(now, idle);
      if (level && mood === "speaking") {
        const { level: l, at } = level.get();
        const target = now - at > LEVEL_STALE_MS ? 0 : l;
        // Opens fast, closes a little slower: syllables read, silence closes it.
        mouth.current += (target - mouth.current) * (target > mouth.current ? 0.45 : 0.2);
        const m = mouth.current;
        f = { ...f, mo: Math.min(1, 0.08 + 1.1 * m), y: (f.y ?? 0) - 0.9 * m, sy: (f.sy ?? 1) * (1 + 0.04 * m), es: (f.es ?? 1) * (1 + 0.06 * m) };
      }
      apply(WISP.attrs(f, d, now));
      if (!motion.settled) raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
  };

  // Clear the id as well: React mounts, unmounts and remounts in development, and a stale id
  // would make `run` think the loop is still going.
  useEffect(
    () => () => {
      cancelAnimationFrame(raf.current);
      raf.current = 0;
    },
    [],
  );

  // Stage 1 on hover, Stage 2 on leave; under reduced motion, the attentive face with no motion.
  useEffect(() => {
    if (frame !== undefined) return;
    if (isStill) {
      cancelAnimationFrame(raf.current);
      raf.current = 0;
      written.current.clear();
      apply(WISP.attrs(stillPose(WISP, hovered, mood), d, 0));
      return;
    }
    motion.hover(hovered, performance.now());
    run();
    if (!hovered) return;
    // Follow the pointer while hovered, measured from the drawing's own center.
    const onMove = (e: PointerEvent) => {
      const r = svgRef.current?.getBoundingClientRect();
      if (!r) return;
      motion.setLookTarget({ x: (e.clientX - (r.left + r.width / 2)) / Math.max(48, r.width * 0.9), y: (e.clientY - (r.top + r.height / 2)) / Math.max(48, r.height * 0.9) });
    };
    window.addEventListener("pointermove", onMove);
    return () => {
      window.removeEventListener("pointermove", onMove);
      motion.setLookTarget({ x: 0, y: 0 });
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hovered, isStill, motion, mood]);

  const lastTrigger = useRef(trigger);
  useEffect(() => {
    if (trigger === lastTrigger.current) return;
    lastTrigger.current = trigger;
    if (isStill) return;
    motion.play(performance.now());
    run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [trigger]);

  useEffect(() => {
    motion.setMood(mood, performance.now());
    if (!isStill) run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mood]);

  // Start (or fade out) the idle loop.
  useEffect(() => {
    if (!isStill) run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [idle, isStill]);

  return (
    <svg
      ref={svgRef}
      width={size}
      height={size}
      viewBox={scene.viewBox}
      className={className}
      style={{ overflow: "visible", display: "block", flex: "none" }}
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
    >
      <defs>
        <mask id={maskId} maskUnits="userSpaceOnUse" x={-24} y={-24} width={72} height={72}>
          <rect x={-24} y={-24} width={72} height={72} fill="#fff" />
          {scene.holes.map((el, i) => draw(el, attrs, i))}
        </mask>
        {lit && (
          // The mark's light (mark.ts litFigure): white in the head, the state's colour at the edge.
          <radialGradient id={`${maskId}-lit`} cx={11.4} cy={9} r={13} gradientUnits="userSpaceOnUse">
            <stop offset={0} stopColor="#ffffff" />
            <stop offset={0.38} stopColor="#f4feff" />
            <stop offset={0.72} stopColor="#eafcff" />
            <stop offset={1} stopColor={lit} />
          </radialGradient>
        )}
      </defs>
      <g data-part="root" transform={attrs.root?.transform as string | undefined}>
        <g mask={`url(#${maskId})`}>{scene.body.map((el, i) => draw(el, attrs, i, lit ? `url(#${maskId}-lit)` : undefined))}</g>
        {scene.over.map((el, i) => draw(el, attrs, i))}
      </g>
      {scene.free.map((el, i) => draw(el, attrs, i))}
    </svg>
  );
}
