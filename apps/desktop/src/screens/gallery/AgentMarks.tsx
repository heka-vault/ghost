/** The agents' marks: every shape at 20 px and 200 px, and every shape in every colour. */
import { AGENT_HUES, AGENT_SHAPES, HUE_NAMES, SHAPE_NAMES } from "@ghost/ui";

import { AgentShapeGlyph, LineGlyph } from "../../ui";

export function AgentMarks() {
  return (
    <section className="gal-block" id="gal-agents">
      <h2 className="gal-h2">Agents’ marks</h2>
      <p className="gal-lead">Wisp’s family: each agent a small ghost with Wisp’s eyes, told apart by shape first and colour second. Every shape at 20 px (the rail, a row) and 200 px (the room), then each in every colour.</p>
      <div className="gal-marks">
        {AGENT_SHAPES.map((shape, i) => (
          <figure key={shape} className="gal-mark">
            <span className="gal-mark__big" style={{ color: `var(--hue-${i % AGENT_HUES}-fg)`, background: `var(--hue-${i % AGENT_HUES}-bg)` }}>
              <AgentShapeGlyph shape={shape} size={200} />
            </span>
            <span className="gal-mark__small">
              <LineGlyph slug={`x-${shape}`} name={shape} shape={shape} hue={i % AGENT_HUES} size="sm" />
              <LineGlyph slug={`x-${shape}`} name={shape} shape={shape} hue={i % AGENT_HUES} size="md" />
              <LineGlyph slug={`x-${shape}`} name={shape} shape={shape} hue={i % AGENT_HUES} size="lg" />
            </span>
            <figcaption>{SHAPE_NAMES[shape]}</figcaption>
          </figure>
        ))}
      </div>
      <table className="gal-mark-grid">
        <thead>
          <tr>
            <th />
            {HUE_NAMES.map((h) => (
              <th key={h}>{h}</th>
            ))}
          </tr>
        </thead>
        <tbody>
          {AGENT_SHAPES.map((shape) => (
            <tr key={shape}>
              <th>{SHAPE_NAMES[shape]}</th>
              {HUE_NAMES.map((h, hue) => (
                <td key={h}>
                  <LineGlyph slug={`x-${shape}`} name={shape} shape={shape} hue={hue} size="lg" />
                </td>
              ))}
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
