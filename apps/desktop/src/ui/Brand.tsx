import { createContext, useContext } from "react";

import { type AgentShape, ARCS, GHOST_PATH, GHOST_PATH_SMALL, MARK_STATES, type MarkState, SHAPES, type VoiceLevel, WISP, lineHue, lookOf } from "@ghost/ui";

import { cx } from "./cx";
import { Mascot } from "./Mascot";

export { GHOST_PATH, GHOST_PATH_SMALL, lineHue };

/**
 * The ghost glyph: Wisp at rest (`WISP.rest`, figure.ts), flat in the current colour, the eyes
 * cut out; the mitts from 32 px, the plain hem under that. No smile, no stick arms: the still
 * mark. An agent drawn as Wisp is this, in its own colour; the lit look is Ghost's alone
 * (`GhostChip`).
 */
export function GhostGlyph({ size = 16, className, title }: { size?: number; className?: string; title?: string }) {
  return <Mascot size={size} frame={WISP.rest} title={title} className={cx("g-ghost-glyph", className)} />;
}

/** Ghost's state as the mark shows it: the light's colour and the ring's lit arc. */
export type { MarkState };

export interface GhostChipProps {
  /** The tile's size in px. */
  size?: number;
  state?: MarkState;
  /** The hairline ring with the state's lit arc (the rail's chip), or none (the welcome's mark). */
  ring?: "arc" | "none";
  /** The live mascot inside: a voice phase's mood, hover, a tap, the idle loop, Ghost's voice. */
  mood?: string | null;
  hovered?: boolean;
  trigger?: number;
  idle?: boolean;
  level?: VoiceLevel;
  className?: string;
  title?: string;
}

/**
 * The mark in the app (docs/design-system.md, The mark): Wisp lit from within on the night tile,
 * the tile's light and, with `ring="arc"`, one arc of a hairline ring in the state's colour (the
 * whole ring while speaking; grey and struck through when not listening). The same drawing as
 * the app's icon (`markSvg`), with the live mascot inside so it can move. On a light page it is
 * a dark chip: never a bare white ghost, never on an indigo slab.
 */
export function GhostChip({ size = 40, state = "idle", ring = "none", mood, hovered, trigger, idle, level, className, title }: GhostChipProps) {
  const tint = MARK_STATES[state].tint;
  const off = state === "off";
  const wisp = Math.round(size * (ring === "arc" ? 0.66 : 0.84));
  const live = mood !== undefined || hovered || (trigger ?? 0) > 0 || idle;
  const [a0, a1] = ARCS[state];
  const R = 39;
  const pt = (deg: number) => [50 + R * Math.cos((deg * Math.PI) / 180), 50 + R * Math.sin((deg * Math.PI) / 180)].map((v) => v.toFixed(2)).join(" ");
  const hair = 100 / size;
  return (
    <span
      className={cx("g-chip", off && "is-off", className)}
      style={{ width: size, height: size, ["--chip-tint" as string]: off ? "#7d8796" : tint }}
      role={title ? "img" : undefined}
      aria-label={title}
      aria-hidden={title ? undefined : true}
    >
      {ring === "arc" && (
        <svg className="g-chip__ring" viewBox="0 0 100 100" aria-hidden>
          <circle cx="50" cy="50" r={R} fill="none" stroke="#eafcff" strokeOpacity={0.3} strokeWidth={hair} />
          {state === "speaking" ? (
            <circle cx="50" cy="50" r={R} fill="none" stroke={tint} strokeWidth={hair * 2} />
          ) : (
            !off && <path d={`M${pt(a0)}A${R} ${R} 0 0 1 ${pt(a1)}`} fill="none" stroke={tint} strokeWidth={hair * 2} strokeLinecap="round" />
          )}
        </svg>
      )}
      <span className="g-chip__wisp">
        {live ? (
          <Mascot size={wisp} lit={off ? "#9aa3b2" : tint} mood={mood} hovered={hovered} trigger={trigger} idle={idle} level={level} />
        ) : (
          <Mascot size={wisp} lit={off ? "#9aa3b2" : tint} frame={WISP.rest} />
        )}
      </span>
      {off && (
        <svg className="g-chip__strike" viewBox="0 0 100 100" aria-hidden>
          <line x1="20" y1="80" x2="80" y2="20" stroke="#05060a" strokeWidth={Math.max(hair * 3.5, 7)} strokeLinecap="round" />
          <line x1="20" y1="80" x2="80" y2="20" stroke="#ff4d5e" strokeWidth={Math.max(hair * 1.6, 3.2)} strokeLinecap="round" />
        </svg>
      )}
    </span>
  );
}

/**
 * The app mark: the lit tile at rest (`GhostChip` with no ring). Pass `hovered` (and
 * optionally `trigger`) to make Wisp in it the live mascot, which plays and settles back.
 */
export function GhostMark({ size = 24, hovered, trigger, className }: { size?: number; hovered?: boolean; trigger?: number; className?: string }) {
  return <GhostChip size={size} hovered={hovered || undefined} trigger={trigger} className={cx("g-mark", className)} />;
}

export interface LineGlyphProps {
  slug: string;
  name: string;
  size?: "sm" | "md" | "lg" | "xl";
  /** Override the hue (0-7). */
  hue?: number;
  /** Override the shape (the picker's previews); else the agent's own look. */
  shape?: AgentShape;
  className?: string;
}

/**
 * Every agent's look, by slug (`"<shape>:<colour>"`, a field on the agent): provided once by
 * the shell from the agents it has loaded, read by every `LineGlyph`, so the rail, Today, the
 * room and every list draw an agent the same way without being handed its look.
 */
export const AgentLooksContext = createContext<ReadonlyMap<string, string | null>>(new Map());

/**
 * Overrides LineGlyph's size for a subtree. Page's compact bar sets "sm" so a header's
 * leading glyph repeats there at 20px without the screen passing two sizes.
 */
export const GlyphSizeContext = createContext<LineGlyphProps["size"] | null>(null);

/**
 * One of the agents' shapes (`@ghost/ui` agentShapes): Wisp's family, its body in the current
 * colour and Wisp's two upright eyes cut in the ground's colour (`--agent-eye`).
 */
export function AgentShapeGlyph({ shape, size = 16, className, title }: { shape: AgentShape; size?: number; className?: string; title?: string }) {
  if (shape === "wisp") return <GhostGlyph size={size} className={className} title={title} />;
  const d = SHAPES[shape];
  return (
    <svg width={size} height={size} viewBox="0 0 24 24" className={cx("g-agent-shape", className)} role={title ? "img" : undefined} aria-label={title} aria-hidden={title ? undefined : true}>
      <path d={d.body} fill="currentColor" />
      {d.eyes.map(([cx_, cy, rx, ry], i) => (
        <ellipse key={i} cx={cx_} cy={cy} rx={rx} ry={ry} fill="var(--agent-eye, var(--bg-canvas))" />
      ))}
    </svg>
  );
}

/**
 * An agent's tile: its shape in its colour on its soft hue (the operator's call, 2026-11-12:
 * shapes as well as colours). Ghost is the mark, the lit tile at the tile's size, as on the
 * phone (`AgentMark`): never Wisp flat in the accent's indigo. An agent without a look is Wisp
 * in the colour its slug picks.
 */
export function LineGlyph({ slug, size: own = "md", hue, shape, className }: LineGlyphProps) {
  const size = useContext(GlyphSizeContext) ?? own;
  const looks = useContext(AgentLooksContext);
  if (slug === "ghost" && hue === undefined && shape === undefined) {
    const tile = { sm: 20, md: 24, lg: 32, xl: 48 }[size];
    // The tile's own corners (22.5%), not an agent tile's radius.
    return <GhostChip size={tile} className={className} />;
  }
  const look = lookOf(slug, looks.get(slug));
  const h = hue ?? look.hue;
  const s = shape ?? look.shape;
  const px = { sm: 14, md: 16, lg: 20, xl: 28 }[size];
  return (
    <span
      className={cx("g-line-glyph", `g-line-glyph--${size}`, className)}
      style={{ color: `var(--hue-${h}-fg)`, background: `var(--hue-${h}-bg)`, ["--agent-eye" as string]: "var(--bg-canvas)" }}
      aria-hidden
    >
      <AgentShapeGlyph shape={s} size={s === "wisp" ? px : Math.round(px * 1.15)} />
    </span>
  );
}
