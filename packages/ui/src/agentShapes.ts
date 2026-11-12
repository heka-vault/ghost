/**
 * Agents' marks: a family around Wisp, Ghost's own mascot. Every agent is a small ghost with
 * Wisp's two upright eyes, told apart by silhouette first and colour second, so two agents in
 * one colour, or a colour-blind operator, still tell them apart at 20px. Drawn on Wisp's 24
 * grid; the silhouettes fill most of it so they read as one family at every size.
 *
 * A look is `"<shape>:<colour>"` (the daemon's `parse_look`): one of `AGENT_SHAPES` and a
 * colour 0 to 7 (`--hue-N-fg`/`--hue-N-bg`). Ghost itself is always Wisp in colour 0. An agent
 * without a look is Wisp in the colour its slug picks (`lineHue`), as before looks existed.
 */
import { lineHue } from "./brand";

export const AGENT_SHAPES = ["wisp", "drop", "bell", "hood", "cloud", "lantern", "star", "moth"] as const;
export type AgentShape = (typeof AGENT_SHAPES)[number];
export const AGENT_HUES = 8;

/** Each shape's name, as the picker says it. */
export const SHAPE_NAMES: Record<AgentShape, string> = {
  wisp: "Wisp",
  drop: "Drop",
  bell: "Bell",
  hood: "Hood",
  cloud: "Cloud",
  lantern: "Lantern",
  star: "Star",
  moth: "Moth",
};

/** The colours' names, as the picker says them (the `--hue-N` tokens, in order). */
export const HUE_NAMES = ["Iris", "Violet", "Rose", "Orange", "Gold", "Green", "Teal", "Sky"] as const;

export interface AgentLook {
  shape: AgentShape;
  hue: number;
}

/** A stored look, or the one an agent gets from its slug. Ghost is always Wisp in colour 0. */
export function lookOf(slug: string, stored?: string | null): AgentLook {
  if (slug === "ghost") return { shape: "wisp", hue: 0 };
  const parsed = parseLook(stored);
  return parsed ?? { shape: "wisp", hue: lineHue(slug) };
}

/** `"lantern:3"` → its look, or null for anything else (as the daemon's `parse_look`). */
export function parseLook(text?: string | null): AgentLook | null {
  if (!text) return null;
  const [shape, hue] = text.split(":").map((s) => s.trim());
  const n = Number(hue);
  if (!(AGENT_SHAPES as readonly string[]).includes(shape) || !Number.isInteger(n) || n < 0 || n >= AGENT_HUES) return null;
  return { shape: shape as AgentShape, hue: n };
}

export function lookText(l: AgentLook): string {
  return `${l.shape}:${l.hue}`;
}

/** A shape's body (filled) and its two eyes, on the 24 grid. Wisp itself is drawn by the mascot. */
export interface ShapeDrawing {
  body: string;
  eyes: [cx: number, cy: number, rx: number, ry: number][];
}

const EYES = (y: number, gap = 2.6, rx = 1.35, ry = 1.95): ShapeDrawing["eyes"] => [
  [12 - gap, y, rx, ry],
  [12 + gap, y, rx, ry],
];

export const SHAPES: Record<Exclude<AgentShape, "wisp">, ShapeDrawing> = {
  // A falling drop: pointed top, round belly.
  drop: { body: "M12 1.8 C 12 1.8 4.6 9.6 4.6 14.6 A 7.4 7.4 0 0 0 19.4 14.6 C 19.4 9.6 12 1.8 12 1.8 Z", eyes: EYES(14.2, 2.5) },
  // A bell: dome and a flared, scalloped skirt.
  bell: { body: "M12 2.6 C 7.6 2.6 6 6 6 10 V 15.6 L 3.6 20.6 Q 5.7 19 7.8 20.6 Q 9.9 19 12 20.6 Q 14.1 19 16.2 20.6 Q 18.3 19 20.4 20.6 L 18 15.6 V 10 C 18 6 16.4 2.6 12 2.6 Z", eyes: EYES(10.6) },
  // A hooded one: a pointed cowl and a jagged hem.
  hood: { body: "M12 1.6 L 17.4 8.4 C 18.6 10 19.2 11.8 19.2 14 V 21 L 16.8 19.2 L 14.4 21 L 12 19.2 L 9.6 21 L 7.2 19.2 L 4.8 21 V 14 C 4.8 11.8 5.4 10 6.6 8.4 Z", eyes: EYES(13.2) },
  // A cloud: three puffs on top, a scalloped bottom.
  cloud: { body: "M5.2 15.4 C 2.4 15.2 1.8 10.8 4.6 10 C 4.4 6.6 8.4 5.2 10.2 7.4 C 11.2 3.8 16.6 3.8 17.2 7.8 C 20.8 7.6 22.4 12.4 19 14.4 V 19.6 Q 17.25 18 15.5 19.6 Q 13.75 18 12 19.6 Q 10.25 18 8.5 19.6 Q 6.85 18 5.2 19.6 Z", eyes: EYES(12.6, 2.7) },
  // A lantern: a handle and a rounded case.
  lantern: { body: "M9 5.4 V 3.9 A 3 3 0 0 1 15 3.9 V 5.4 H 13.4 V 3.9 A 1.4 1.4 0 0 0 10.6 3.9 V 5.4 Z M7.6 4.8 H 16.4 A 2.6 2.6 0 0 1 19 7.4 V 18.4 A 2.6 2.6 0 0 1 16.4 21 H 7.6 A 2.6 2.6 0 0 1 5 18.4 V 7.4 A 2.6 2.6 0 0 1 7.6 4.8 Z", eyes: EYES(12) },
  // A star with soft points.
  star: { body: "M12 1.6 L 14.9 8.2 L 22 8.8 L 16.6 13.4 L 18.3 20.4 L 12 16.7 L 5.7 20.4 L 7.4 13.4 L 2 8.8 L 9.1 8.2 Z", eyes: EYES(11.6, 1.9, 1.05, 1.55) },
  // A moth: a round body with a wing either side.
  moth: { body: "M12 4.6 C 15 4.6 16.6 8.4 16.6 12.6 C 16.6 17.4 14.6 20.8 12 20.8 C 9.4 20.8 7.4 17.4 7.4 12.6 C 7.4 8.4 9 4.6 12 4.6 Z M9 10.4 C 3.2 5.6 0.8 10.6 2.6 14.4 C 3.8 16.8 7.2 16.8 9 14.8 Z M15 10.4 C 20.8 5.6 23.2 10.6 21.4 14.4 C 20.2 16.8 16.8 16.8 15 14.8 Z", eyes: EYES(11.4, 2, 1.15, 1.7) },
};
