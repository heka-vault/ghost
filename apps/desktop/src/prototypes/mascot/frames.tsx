/**
 * Every frame of Wisp, flat: run through the real motion machine on a simulated clock and
 * sampled into strips. Review the poses here, not by squinting at a hover.
 */
import { createMotion, type Look, type MascotFrame as Frame, WISP } from "@ghost/ui";

import { Mascot } from "../../ui";

interface Script {
  title: string;
  /** [ms, event] in order. */
  events: [number, "hover" | "unhover" | "tap"][];
  look?: Look;
  samples: number[];
}

const range = (from: number, to: number, step: number) => Array.from({ length: Math.floor((to - from) / step) + 1 }, (_, i) => from + i * step);

const SCRIPTS: Script[] = [
  { title: "Hover, stay, leave at 2400", events: [[0, "hover"], [2400, "unhover"]], look: { x: 0.6, y: -0.2 }, samples: [...range(0, 1200, 80), 1600, 2000, 2400, 2480, 2560, 2640, 2760, 2900] },
  { title: "A quick pass: leave at 150 ms (the gesture finishes, then settles)", events: [[0, "hover"], [150, "unhover"]], samples: range(0, 2000, 100) },
  { title: "Tap (the phone)", events: [[0, "tap"]], samples: range(0, 3200, 160) },
  { title: "Back again mid-settle: leave at 1400, return at 1560", events: [[0, "hover"], [1400, "unhover"], [1560, "hover"]], samples: range(1360, 2400, 40) },
];

const LOOKS: Look[] = [
  { x: -1, y: -1 }, { x: 0, y: -1 }, { x: 1, y: -1 },
  { x: -1, y: 0 }, { x: 0, y: 0 }, { x: 1, y: 0 },
  { x: -1, y: 1 }, { x: 0, y: 1 }, { x: 1, y: 1 },
];

function film(s: Script): { t: number; f: Frame }[] {
  const m = createMotion(WISP);
  const out: { t: number; f: Frame }[] = [];
  const last = s.samples[s.samples.length - 1];
  const events = [...s.events];
  const samples = new Set(s.samples);
  // Real frame loops tick about every 16 ms; sample on that grid.
  for (let t = 0; t <= last; t += 4) {
    while (events.length && events[0][0] <= t) {
      const [, e] = events.shift()!;
      if (e === "tap") m.play(t);
      else m.hover(e === "hover", t);
    }
    if (s.look) m.setLookTarget(s.look);
    const f = m.frame(t, false);
    if (samples.has(t)) out.push({ t, f });
  }
  return out;
}

/** A mood from rest: the blend in, then its loop, sampled every 160ms. */
function moodFilm(mood: string): { t: number; f: Frame }[] {
  const m = createMotion(WISP);
  const out: { t: number; f: Frame }[] = [];
  m.setMood(mood, 0);
  for (let t = 0; t <= 3360; t += 4) {
    const f = m.frame(t, false);
    if (t % 160 === 0) out.push({ t, f });
  }
  return out;
}

export function Frames() {
  return (
    <div className="masc-frames">
      {SCRIPTS.map((s) => (
        <Strip key={s.title} title={s.title} frames={film(s)} />
      ))}
      {Object.keys(WISP.moods).map((mood) => (
        <Strip key={mood} title={`Mood: ${mood}, blending in from rest`} frames={moodFilm(mood)} />
      ))}
      <Strip
        title="Holding, looking every way (bounds of the eyes and arms)"
        frames={LOOKS.map((look, i) => ({ t: i, f: WISP.hold(900, { look }), label: `${look.x},${look.y}` }))}
      />
    </div>
  );
}

function Strip({ title, frames }: { title: string; frames: { t: number; f: Frame; label?: string }[] }) {
  return (
    <div className="masc-strip">
      <h3 className="t-subheading">{title}</h3>
      <div className="masc-strip__frames">
        {frames.map(({ t, f, label }) => (
          <figure key={t} className="masc-frame">
            <Mascot size={96} frame={f} clock={t} />
            <span className="masc-frame__small">
              <Mascot size={24} frame={f} clock={t} />
              <Mascot size={16} frame={f} clock={t} />
            </span>
            <figcaption className="t-caption t-3 t-num">{label ?? `${t} ms`}</figcaption>
          </figure>
        ))}
      </div>
    </div>
  );
}
