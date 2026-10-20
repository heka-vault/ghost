/**
 * Mascot workbench (#/mascot): Wisp at every size and on every tile, live, with the idle loop and
 * reduced motion to toggle; every frame of its motion below (#/mascot/frames on its own).
 */
import { type ReactNode, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { Badge, Button, Card, Mascot, Page, Section, SegmentedControl, Switch } from "../../ui";
import { Frames } from "./frames";
import "./mascot.css";

export default function MascotPrototype({ params }: RouteProps) {
  const [idle, setIdle] = useState(false);
  const [still, setStill] = useState(false);
  const [hovered, setHovered] = useState(false);
  const [trigger, setTrigger] = useState(0);
  const [mood, setMood] = useState<Mood>("passive");
  // Off follows prefers-reduced-motion rather than overriding it.
  const common = { hovered, trigger, idle, mood, still: still || undefined };
  const framesOnly = params.view === "frames";
  return (
    <Page
      width="wide"
      title="Mascot"
      eyebrow={<Badge size="sm" variant="outline">Prototype</Badge>}
      description="Wisp, Ghost’s mascot. Point at the card to play it the way the desktop app does; Play as a tap plays it the way the phone does."
      actions={
        <div className="masc-toggles">
          <Switch checked={idle} onChange={setIdle} label="Idle loop" size="sm" />
          <Switch checked={still} onChange={setStill} label="Reduced motion" size="sm" />
        </div>
      }
    >
      {!framesOnly && (
        <Card padding="none" className="masc-card" onPointerEnter={(e) => e.pointerType === "mouse" && setHovered(true)} onPointerLeave={() => setHovered(false)}>
          <div className="masc-stage" style={{ color: `var(--phase-${mood === "hold" ? "passive" : mood})` }}>
            <Mascot {...common} size={256} title="Wisp, Ghost’s mascot" />
          </div>
          <div className="masc-moods">
            <SegmentedControl aria-label="Voice phase" size="sm" options={MOODS.map((m) => ({ value: m, label: MOOD_LABEL[m] }))} value={mood} onChange={setMood} />
          </div>
          <div className="masc-body">
            <div className="masc-row" aria-label="Sizes">
              {[16, 24, 48, 96].map((s) => (
                <figure key={s} className="masc-size">
                  <Mascot {...common} size={s} />
                  <figcaption className="t-caption t-3 t-num">{s}</figcaption>
                </figure>
              ))}
              <figure className="masc-size">
                <span className="g-line-glyph g-line-glyph--lg" style={{ color: "var(--hue-0-fg)", background: "var(--hue-0-bg)" }}><Mascot {...common} size={20} /></span>
                <figcaption className="t-caption t-3">Line</figcaption>
              </figure>
              <figure className="masc-size">
                <Tile size={28}><Mascot {...common} size={18} /></Tile>
                <figcaption className="t-caption t-3">Mark 28</figcaption>
              </figure>
              <figure className="masc-size">
                <Tile size={64}><Mascot {...common} size={56} /></Tile>
                <figcaption className="t-caption t-3">Mark 64</figcaption>
              </figure>
              <figure className="masc-size">
                <span className="masc-theme" data-theme="light"><Mascot {...common} size={32} /></span>
                <figcaption className="t-caption t-3">Light</figcaption>
              </figure>
              <figure className="masc-size">
                <span className="masc-theme" data-theme="dark"><Mascot {...common} size={32} /></span>
                <figcaption className="t-caption t-3">Dark</figcaption>
              </figure>
            </div>
            <div>
              <Button size="sm" variant="secondary" onClick={() => setTrigger((t) => t + 1)}>Play as a tap</Button>
            </div>
          </div>
        </Card>
      )}
      <Section title="Every frame" description="The real motion machine on a simulated clock. Large frames are 96px; the pair under each is 24 and 16.">
        <Frames />
      </Section>
    </Page>
  );
}

const MOODS = ["passive", "starting", "listening", "thinking", "speaking", "hold", "offline"] as const;
type Mood = (typeof MOODS)[number];
const MOOD_LABEL: Record<Mood, string> = { passive: "Not active", starting: "Starting", listening: "Listening", thinking: "Thinking", speaking: "Speaking", hold: "Checked out", offline: "Offline" };

/** The app mark's accent tile, as GhostMark draws it, holding the live mascot. */
function Tile({ size, children }: { size: number; children: ReactNode }) {
  return <span className="g-mark" style={{ width: size, height: size, borderRadius: Math.round(size * 0.28) }}>{children}</span>;
}
