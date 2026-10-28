/**
 * Owner: settings. Charter settings with no API yet: privacy (on General) and sounds (on
 * Sound). Loaded only in development and rendered only while the mock layer is on (index.tsx
 * gates both), so a production build never contains these controls. Values live in this
 * component's state.
 */
import { useState } from "react";

import { Badge, Card, Checkbox, Field, Section, Select, Slider, Switch } from "../../ui";
import { sectionDomId } from "./model";
import { Row } from "./parts";

const PrototypeTag = () => (
  <Badge tone="info" size="sm" className="set-proto-tag">
    Prototype
  </Badge>
);

const TRANSCRIPT_DAYS = [
  { value: "7", label: "7 days" },
  { value: "30", label: "30 days" },
  { value: "90", label: "90 days" },
  { value: "0", label: "Until I delete them" },
];
const AUDIT_DAYS = [
  { value: "90", label: "90 days" },
  { value: "365", label: "1 year" },
  { value: "0", label: "Forever" },
];
const DUCK_APPS = [
  { id: "spotify", name: "Spotify", detail: "Music" },
  { id: "firefox", name: "Firefox", detail: "Videos and tabs with sound" },
  { id: "zoom", name: "Zoom", detail: "Calls" },
  { id: "slack", name: "Slack", detail: "Huddles and notification sounds" },
];

export default function SettingsPrototype({ only }: { only: "privacy" | "sounds" }) {
  const [transcripts, setTranscripts] = useState("30");
  const [audit, setAudit] = useState("365");
  const [indicator, setIndicator] = useState(true);
  const [chime, setChime] = useState(true);
  const [thinking, setThinking] = useState(false);
  const [speed, setSpeed] = useState(1);
  const [volume, setVolume] = useState(80);
  const [duck, setDuck] = useState<Record<string, boolean>>({ spotify: true, firefox: true, zoom: false, slack: false });

  if (only === "privacy")
    return (
      <Section
        id={sectionDomId("privacy")}
        className="set-section"
        title={<span className="set-title-tag">Privacy <PrototypeTag /></span>}
        description="How long Ghost keeps what it hears and does. Not saved yet."
      >
        <Card list>
          <Row>
            <Field layout="inline" label="Keep session transcripts" help="What you and Ghost said. Idle audio is never kept.">
              <Select value={transcripts} onChange={(e) => setTranscripts(e.target.value)} options={TRANSCRIPT_DAYS} />
            </Field>
          </Row>
          <Row>
            <Field layout="inline" label="Keep the audit log" help="Every change Ghost made or asked to make.">
              <Select value={audit} onChange={(e) => setAudit(e.target.value)} options={AUDIT_DAYS} />
            </Field>
          </Row>
          <Row>
            <Field layout="inline" label="Show when the microphone is live" help="A dot in the sidebar and the tray while Ghost is listening.">
              <span className="set-switch-end">
                <Switch checked={indicator} onChange={setIndicator} />
              </span>
            </Field>
          </Row>
        </Card>
      </Section>
    );

  return (
    <Section
      id={sectionDomId("sounds")}
      className="set-section"
      title={<span className="set-title-tag">Sounds <PrototypeTag /></span>}
      description="How Ghost sounds while it works. Not saved yet."
    >
      <Card list>
        <Row>
          <Field layout="inline" label="Wake chime" help="A short tone when Ghost hears its name.">
            <span className="set-switch-end">
              <Switch checked={chime} onChange={setChime} />
            </span>
          </Field>
        </Row>
        <Row>
          <Field layout="inline" label="Thinking sound" help="A soft pulse while the model works on a long answer.">
            <span className="set-switch-end">
              <Switch checked={thinking} onChange={setThinking} />
            </span>
          </Field>
        </Row>
        <Row>
          <Field layout="inline" label="Speaking speed" help="Each agent can set its own; this is the default.">
            <Slider value={speed} onChange={setSpeed} min={0.8} max={1.4} step={0.05} format={(v) => `${v.toFixed(2)}×`} />
          </Field>
        </Row>
        <Row>
          <Field layout="inline" label="Voice volume" help="Relative to your system volume.">
            <Slider value={volume} onChange={setVolume} min={0} max={100} step={5} format={(v) => `${v}%`} />
          </Field>
        </Row>
        <Row className="set-duck">
          <div className="set-duck__head">
            <p className="set-label">Lower these apps while Ghost talks</p>
            <p className="set-help">They go back to their own volume when Ghost stops.</p>
          </div>
          <div className="set-duck__list" role="group" aria-label="Apps to lower while Ghost talks">
            {DUCK_APPS.map((a) => (
              <Checkbox key={a.id} checked={!!duck[a.id]} onChange={(on) => setDuck({ ...duck, [a.id]: on })} label={a.name} description={a.detail} />
            ))}
          </div>
        </Row>
      </Card>
    </Section>
  );
}
