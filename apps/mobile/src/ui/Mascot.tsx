/**
 * Wisp, Ghost's mascot, as on desktop (apps/desktop/src/ui/Mascot.tsx): the same character and
 * motion machine from @ghost/ui, drawn with react-native-svg. Bump `trigger` to play it once (a
 * tap). There is no hover on a phone, so it looks up at you while it plays.
 *
 * `mood` is what Wisp is doing between gestures: a voice phase (listening, thinking, speaking,
 * hold, starting, offline), blended from one to the next. The frame loop runs only while
 * something moves; at rest nothing ticks. A mood alone redraws at 30 frames a second, which its
 * slow motions don't need more than, and a gesture at full rate. `blinks` adds a blink every few
 * seconds at rest, for a placement that should look alive (Home's Start control) without
 * redrawing every frame: the float around it moves on the native driver. Reduced motion shows the
 * mood's one still pose and never moves.
 */
import { createMotion, detailFor, type MascotAttrs, type MascotEl, type MascotFrame, stillPose, WISP } from "@ghost/ui";
import { createElement, type ReactNode, useEffect, useId, useMemo, useRef, useState } from "react";
import Svg, { Circle, Defs, Ellipse, G, Line, Mask, Path, Rect } from "react-native-svg";

import { useReducedMotion } from "./motion";

const TAGS = { path: Path, ellipse: Ellipse, circle: Circle, line: Line, g: G } as const;

/** A mood alone redraws no faster than this. */
const MOOD_FRAME_MS = 33;
/** A blink every ~4.5s, jittered so it never feels mechanical; each lasts 170ms. */
const BLINK_EVERY_MS = 4500;
const BLINK_MS = 170;

function blinkCurve(u: number): number {
  if (u < 0 || u >= BLINK_MS) return 0;
  return u < 60 ? u / 60 : 1 - (u - 60) / 110;
}

function draw(el: MascotEl, attrs: MascotAttrs, key: number): ReactNode {
  const live = el.part ? attrs[el.part] : undefined;
  return createElement(TAGS[el.tag] as never, { key, ...el.props, ...live }, el.children?.map((c, i) => draw(c, attrs, i)));
}

export interface MascotProps {
  /** Pixel size of the square. Under 24 draws the simple body; 40 and up adds the arms. */
  size?: number;
  color: string;
  /** Increment to play once. */
  trigger?: number;
  /** Blink now and then while at rest. */
  blinks?: boolean;
  /** What Wisp is doing between gestures: a voice phase. */
  mood?: string | null;
  /** Draw this exact pose and never animate (an icon). */
  frame?: MascotFrame;
}

export function Mascot({ size = 96, color, trigger = 0, blinks = false, mood = null, frame }: MascotProps) {
  const still = useReducedMotion();
  const { simple, props } = detailFor(size);
  const d = useMemo(() => ({ simple, props }), [simple, props]);
  const scene = useMemo(() => WISP.scene(d), [d]);
  const maskId = `mascot-${useId().replace(/[^a-zA-Z0-9_-]/g, "")}`;
  const motion = useMemo(() => createMotion(WISP), []);
  const [view, setView] = useState<{ f: MascotFrame; clock: number }>({ f: WISP.rest, clock: 0 });
  const raf = useRef(0);
  const blinkAt = useRef(-1);
  const drawnAt = useRef(-Infinity);

  // One loop for gestures and blinks. Time comes from performance.now(), the same clock the
  // machine was started on, rather than from the frame callback's argument.
  const run = () => {
    if (raf.current) return;
    const step = () => {
      raf.current = 0;
      const now = performance.now();
      let f = motion.frame(now, false);
      const b = blinkAt.current >= 0 ? blinkCurve(now - blinkAt.current) : 0;
      if (blinkAt.current >= 0 && now - blinkAt.current >= BLINK_MS) blinkAt.current = -1;
      if (b > 0) f = { ...f, bl: Math.max(f.bl, b) };
      const busy = !motion.settled || blinkAt.current >= 0;
      // Full rate for a gesture or a blink, 30 a second for a mood alone. The last frame of a
      // settle is always drawn, so it lands exactly at rest.
      if (motion.gesturing || b > 0 || !busy || now - drawnAt.current >= MOOD_FRAME_MS) {
        drawnAt.current = now;
        setView({ f, clock: now });
      }
      if (busy) raf.current = requestAnimationFrame(step);
    };
    raf.current = requestAnimationFrame(step);
  };

  // Clear the id as well as cancelling: a remount must be able to start the loop again.
  useEffect(
    () => () => {
      cancelAnimationFrame(raf.current);
      raf.current = 0;
    },
    [],
  );

  const fixed = frame !== undefined;
  const lastTrigger = useRef(trigger);
  useEffect(() => {
    if (trigger === lastTrigger.current) return;
    lastTrigger.current = trigger;
    if (still || fixed) return;
    motion.play(performance.now());
    run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [trigger]);

  useEffect(() => {
    if (fixed) return;
    motion.setMood(mood, performance.now());
    if (!still) run();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [mood, still]);

  useEffect(() => {
    if (!blinks || still || fixed) return;
    let timer: ReturnType<typeof setTimeout>;
    const next = () => {
      timer = setTimeout(() => {
        blinkAt.current = performance.now();
        run();
        next();
      }, BLINK_EVERY_MS * (0.7 + Math.random() * 0.6));
    };
    next();
    return () => clearTimeout(timer);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [blinks, still]);

  const attrs = WISP.attrs(frame ? { ...WISP.rest, ...frame } : still ? stillPose(WISP, false, mood) : view.f, d, view.clock);
  return (
    <Svg width={size} height={size} viewBox={scene.viewBox} color={color}>
      <Defs>
        <Mask id={maskId} maskUnits="userSpaceOnUse" x={-24} y={-24} width={72} height={72}>
          <Rect x={-24} y={-24} width={72} height={72} fill="#fff" />
          {scene.holes.map((el, i) => draw(el, attrs, i))}
        </Mask>
      </Defs>
      <G transform={attrs.root?.transform as string | undefined}>
        <G mask={`url(#${maskId})`}>{scene.body.map((el, i) => draw(el, attrs, i))}</G>
        {scene.over.map((el, i) => draw(el, attrs, i))}
      </G>
      {scene.free.map((el, i) => draw(el, attrs, i))}
    </Svg>
  );
}
