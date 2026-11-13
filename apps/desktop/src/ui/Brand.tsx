import { createContext, useContext } from "react";

import { type AgentShape, GHOST_PATH, GHOST_PATH_SMALL, SHAPES, lineHue, lookOf, wavePose } from "@ghost/ui";

import { cx } from "./cx";
import { Mascot } from "./Mascot";

export { GHOST_PATH, GHOST_PATH_SMALL, lineHue };

/**
 * The ghost glyph: Wisp saying hello (grinning, mid-wave), drawn still, at every size. The
 * mascot only draws its arms at 40px and up, so smaller glyphs are the 40px drawing scaled
 * down: the plain body at small sizes read as the old icon (the operator, 2026-10-20).
 */
const GLYPH_MIN = 40;
export function GhostGlyph({ size = 16, className, title }: { size?: number; className?: string; title?: string }) {
  if (size >= GLYPH_MIN) return <Mascot size={size} frame={wavePose()} title={title} className={cx("g-ghost-glyph", className)} />;
  const k = size / GLYPH_MIN;
  return (
    <span className={cx("g-ghost-glyph", className)} style={{ display: "inline-block", width: size, height: size, overflow: "visible", lineHeight: 0 }} aria-hidden={title ? undefined : true} role={title ? "img" : undefined} aria-label={title}>
      <span style={{ display: "inline-block", transform: `scale(${k})`, transformOrigin: "top left", width: GLYPH_MIN, height: GLYPH_MIN, lineHeight: 0 }}>
        <Mascot size={GLYPH_MIN} frame={wavePose()} />
      </span>
    </span>
  );
}

/**
 * The app mark: Wisp saying hello, bare, no tile (the tile read as the old icon; the
 * operator, 2026-10-20). Pass `hovered` (and optionally `trigger`) to make it the live
 * mascot, which plays and settles back to this mark.
 */
export function GhostMark({ size = 24, hovered, trigger, className }: { size?: number; hovered?: boolean; trigger?: number; className?: string }) {
  const glyph = size;
  return (
    <span className={cx("g-mark", className)} style={{ width: size, height: size }} aria-hidden>
      {/* Still: Wisp saying hello at any size. While hovered (or playing), the live mascot. */}
      {hovered || (trigger !== undefined && trigger > 0) ? <Mascot size={glyph} hovered={hovered} trigger={trigger} /> : <GhostGlyph size={glyph} />}
    </span>
  );
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
 * shapes as well as colours). Ghost is Wisp; an agent without a look is Wisp in the colour its
 * slug picks.
 */
export function LineGlyph({ slug, size: own = "md", hue, shape, className }: LineGlyphProps) {
  const size = useContext(GlyphSizeContext) ?? own;
  const looks = useContext(AgentLooksContext);
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
