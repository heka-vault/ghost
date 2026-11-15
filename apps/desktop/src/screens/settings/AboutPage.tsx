/**
 * Owner: settings. Settings › About (ADR 0073), for everyone: what Ghost is (the version and
 * build, this window's), its updates (moved from General, the same state, `UpdatesSection`),
 * the site and the README's Privacy section, the licences of what Ghost is built from, and
 * “Save a diagnostic report” (Diagnostic.tsx).
 *
 * The version row counts clicks, Enter and Space alike (app/developer.ts): from the fourth a
 * hint says how many are left, in a live region a screen reader hears, and the seventh adds
 * Developer to the settings nav. Not a lock, and never said to be one.
 *
 * States by URL (mock): `?about=taps` holds the count at four (the hint showing),
 * `?about=report` opens the report's summary.
 */
import type { Status } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ExternalLink } from "lucide-react";
import { type KeyboardEvent, useRef, useState } from "react";

import { NO_TAPS, type Taps, developerOn, setDeveloper, tap } from "../../app/developer";
import { settingsPath } from "../../app/routes";
import { inTauri } from "../../transport";
import { Button, Card, ListRow, Section, buildText, useToast } from "../../ui";
import { VersionNotice, VersionRows } from "./About";
import { DiagnosticReport } from "./Diagnostic";
import { Licences } from "./Licences";
import { SECTION_TITLE, sectionDomId } from "./model";
import { UpdatesSection } from "./Updates";

export const SITE = "https://heyghost.app";
export const PRIVACY = "https://github.com/heka-vault/ghost#privacy";

/** Open an address in the person's browser. */
export async function openLink(url: string) {
  if (!inTauri) {
    window.open(url, "_blank", "noopener");
    return;
  }
  const { openUrl } = await import("@tauri-apps/plugin-opener");
  await openUrl(url);
}

/** The version, clickable seven times for Developer. */
function VersionTap({ status, forced }: { status: Status | null; forced: boolean }) {
  const toast = useToast();
  const taps = useRef<Taps>(forced ? { count: 4, at: Date.now() } : NO_TAPS);
  const [hint, setHint] = useState<string | null>(forced ? "3 more clicks for developer settings." : null);
  const press = () => {
    const r = tap(taps.current, Date.now(), developerOn());
    taps.current = r.taps;
    setHint(r.hint);
    if (r.unlock) {
      setDeveloper(true);
      toast({
        title: "Developer settings are on",
        description: "Settings › Developer: advanced pages, experimental switches and debug mode. Switch it off there.",
        tone: "ok",
        action: { label: "Open", onClick: () => (window.location.hash = `#${settingsPath("developer")}`) },
      });
    }
  };
  const onKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    // A button already clicks on Enter and Space; only a held key's repeats are dropped.
    if (e.repeat && (e.key === "Enter" || e.key === " ")) e.preventDefault();
  };
  const version = status?.version ?? "–";
  return (
    <span className="set-version-tap">
      <button type="button" className="set-version-tap__button set-value is-mono" onClick={press} onKeyDown={onKey} aria-describedby="set-version-hint">
        {version}
        {status?.build ? <span className="sr-only">, {buildText(status.build)}</span> : null}
      </button>
      <span id="set-version-hint" className="set-version-tap__hint" role="status" aria-live="polite">
        {hint ?? ""}
      </span>
    </span>
  );
}

export function AboutPage({ forced }: { forced: string | null }) {
  const { status } = useGhost();
  return (
    <>
      <Section id={sectionDomId("about")} title={SECTION_TITLE.about} className="set-section">
        <VersionNotice status={status} />
        <Card list>
          <ListRow title="Version" description="The Ghost running on this computer." meta={<VersionTap status={status} forced={forced === "taps"} />} />
          <VersionRows status={status} only="rest" />
          <ListRow title="Website" description="What Ghost is, and how to get it." trailing={<Button size="sm" variant="ghost" trailingIcon={<ExternalLink />} onClick={() => void openLink(SITE)}>heyghost.app</Button>} />
          <ListRow
            title="Privacy"
            description="What stays on this computer, what leaves it and when, in the README’s own words."
            trailing={<Button size="sm" variant="ghost" trailingIcon={<ExternalLink />} onClick={() => void openLink(PRIVACY)}>Read it</Button>}
          />
        </Card>
      </Section>
      <UpdatesSection />
      <DiagnosticReport openOnLoad={forced === "report"} />
      <Licences />
    </>
  );
}
