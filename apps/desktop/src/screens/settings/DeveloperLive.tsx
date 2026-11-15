/**
 * Owner: settings. The slot for Developer's live views, built after ADR 0073 merges: a live
 * timing overlay (each turn's stages as they happen) and a live microphone meter. Each is a
 * component boundary here and a place at #/settings/developer/live; until they are built
 * they say so and show no data, never a made-up number.
 *
 * To build one: replace its `ComingNext` body with the view, reading the daemon's events
 * (`useGhost`), and keep its title and section id.
 */
import { Activity, Mic } from "lucide-react";

import { hrefFor } from "../../app/router";
import { Badge, Card, ListRow, Section } from "../../ui";

/** The live timing overlay: coming next. */
export function LiveTimingSlot() {
  return <ListRow leading={<Activity size={16} aria-hidden />} title="Live timing overlay" description="Each turn’s stages as they happen: hearing, the model’s first word, Ghost’s first sound." meta={<Badge size="sm">Coming next</Badge>} />;
}

/** The live microphone meter: coming next. */
export function MicMeterSlot() {
  return <ListRow leading={<Mic size={16} aria-hidden />} title="Live microphone meter" description="What Ghost’s microphone hears, as a level, and what the speech detector makes of it." meta={<Badge size="sm">Coming next</Badge>} />;
}

/** The two slots, on Developer. */
export function DeveloperComingNext() {
  return (
    <Section id="set-live" title="Coming next" description="Live views for testing, not built yet." className="set-section">
      <Card list>
        <LiveTimingSlot />
        <MicMeterSlot />
        <ListRow title="Their page" description="Where they will open, full width." href={hrefFor("/settings/developer/live")} chevron />
      </Card>
    </Section>
  );
}

/** #/settings/developer/live: the place the live views will fill. */
export function DeveloperLivePage() {
  return (
    <Section id="set-live-page" title="Live views" description="Coming next: nothing here is measured yet." className="set-section">
      <Card list>
        <LiveTimingSlot />
        <MicMeterSlot />
      </Card>
    </Section>
  );
}
