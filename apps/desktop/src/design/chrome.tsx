/**
 * Design-mode chrome: the sidebar's Mock badge and scenario switcher, and the prototype strip
 * over prototype routes. Loaded lazily by the shell in development only (`import.meta.env.DEV`),
 * so production builds carry none of these strings.
 */
import { FlaskConical } from "lucide-react";

import { navigate } from "../app/router";
import { ROUTES } from "../app/routes";
import { Badge, IconButton, Menu, Select, Tooltip } from "../ui";
import { useDesign } from "./context";

export function MockBadge() {
  const design = useDesign();
  if (!design) return null;
  return (
    <Tooltip content={`Mock data: ${design.scenario.label}`} side="bottom">
      <Badge tone="accent" size="sm" icon={<FlaskConical />}>
        Mock
      </Badge>
    </Tooltip>
  );
}

export function ScenarioSwitcher() {
  const design = useDesign();
  if (!design) return null;
  return (
    <div className="sidebar__scenario">
      <Select
        size="sm"
        aria-label="Mock scenario"
        value={design.scenario.name}
        onChange={(e) => design.setScenario(e.target.value)}
        options={design.scenarios.map((s) => ({ value: s.name, label: `Scenario: ${s.label}` }))}
      />
    </div>
  );
}

/**
 * Over a prototype route. With the mock active it says so; against the real daemon it says
 * the opposite, in warn tone, because a prototype that saves (onboarding) saves for real.
 */
export function PrototypeStrip() {
  const design = useDesign();
  return (
    <div className={design ? "shell-proto-flag" : "shell-proto-flag is-real"} role={design ? undefined : "status"}>
      <Badge tone={design ? "accent" : "warn"} size="sm">
        Prototype
      </Badge>
      <span>{design ? "Mock data only. Not connected to Ghost." : "Connected to your Ghost. Saves here are real."}</span>
    </div>
  );
}

/**
 * The rail's design-mode menu (development only): which mock scenario, and the prototypes,
 * which no longer have rows of their own. The flask is the Mock badge: it says the data is fake.
 */
export function DevMenu() {
  const design = useDesign();
  if (!design) return null;
  const protos = ROUTES.filter((r) => r.component && r.nav?.group === "prototype");
  return (
    <Menu
      align="start"
      aria-label="Design mode"
      trigger={<IconButton className="rail__dev" label={`Mock data: ${design.scenario.label}`} tooltipSide="right" icon={<FlaskConical />} />}
      items={[
        { heading: "Scenario" },
        ...design.scenarios.map((s) => ({ label: s.name === design.scenario.name ? `${s.label} ✓` : s.label, onSelect: () => design.setScenario(s.name) })),
        "separator" as const,
        { heading: "Prototypes" },
        ...protos.map((r) => ({ label: r.nav!.label, onSelect: () => navigate(r.path) })),
      ]}
    />
  );
}
