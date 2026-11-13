/**
 * Choosing how an agent looks: its shape (Wisp's family) and its colour, both radio groups
 * (arrow keys move and choose, Tab leaves), previewed at the size the rail draws it and large.
 * Used when hiring and in the agent's own settings; the choice is a field on the agent
 * (`look`, `"<shape>:<colour>"`), saved like its voice.
 */
import { AGENT_SHAPES, type AgentLook, HUE_NAMES, SHAPE_NAMES } from "@ghost/ui";
import { type KeyboardEvent, useId } from "react";

import { AgentShapeGlyph, LineGlyph, cx } from "../../ui";

export function LookPicker({ value, onChange, name, disabled }: { value: AgentLook; onChange: (l: AgentLook) => void; name: string; disabled?: boolean }) {
  const id = useId();
  const roving = (n: number, at: number, pick: (i: number) => void) => (e: KeyboardEvent) => {
    const step = e.key === "ArrowRight" || e.key === "ArrowDown" ? 1 : e.key === "ArrowLeft" || e.key === "ArrowUp" ? -1 : 0;
    if (!step) return;
    e.preventDefault();
    const next = (at + step + n) % n;
    pick(next);
    const group = (e.currentTarget as HTMLElement).parentElement;
    requestAnimationFrame(() => (group?.children[next] as HTMLElement | undefined)?.focus());
  };
  return (
    <div className="conv-look">
      <span className="conv-look__preview" style={{ color: `var(--hue-${value.hue}-fg)`, background: `var(--hue-${value.hue}-bg)` }} aria-hidden>
        <AgentShapeGlyph shape={value.shape} size={72} />
      </span>
      <div className="conv-look__choices">
        <p className="conv-look__label" id={`${id}-shape`}>
          Shape <span className="t-3">· {SHAPE_NAMES[value.shape]}</span>
        </p>
        <div className="conv-look__row" role="radiogroup" aria-labelledby={`${id}-shape`}>
          {AGENT_SHAPES.map((s, i) => (
            <button
              key={s}
              type="button"
              role="radio"
              aria-checked={s === value.shape}
              aria-label={SHAPE_NAMES[s]}
              title={SHAPE_NAMES[s]}
              tabIndex={s === value.shape ? 0 : -1}
              disabled={disabled}
              className={cx("conv-look__opt", s === value.shape && "is-on")}
              onClick={() => onChange({ ...value, shape: s })}
              onKeyDown={roving(AGENT_SHAPES.length, i, (n) => onChange({ ...value, shape: AGENT_SHAPES[n] }))}
            >
              <LineGlyph slug={`look-${s}`} name={name} shape={s} hue={value.hue} size="lg" />
            </button>
          ))}
        </div>
        <p className="conv-look__label" id={`${id}-hue`}>
          Colour <span className="t-3">· {HUE_NAMES[value.hue]}</span>
        </p>
        <div className="conv-look__row" role="radiogroup" aria-labelledby={`${id}-hue`}>
          {HUE_NAMES.map((h, hue) => (
            <button
              key={h}
              type="button"
              role="radio"
              aria-checked={hue === value.hue}
              aria-label={h}
              title={h}
              tabIndex={hue === value.hue ? 0 : -1}
              disabled={disabled}
              className={cx("conv-look__swatch", hue === value.hue && "is-on")}
              style={{ background: `var(--hue-${hue}-fg)` }}
              onClick={() => onChange({ ...value, hue })}
              onKeyDown={roving(HUE_NAMES.length, hue, (n) => onChange({ ...value, hue: n }))}
            />
          ))}
        </div>
      </div>
    </div>
  );
}

/** The slug a name becomes, as the daemon's `slugify` makes it: what the default look is picked from. */
export function slugOf(name: string): string {
  const s = name
    .toLowerCase()
    .normalize("NFKD")
    .replace(/[^a-z0-9]+/g, "-")
    .replace(/^-+|-+$/g, "")
    .slice(0, 40);
  return s || "agent";
}
