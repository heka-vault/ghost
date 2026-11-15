/**
 * Owner: settings. Error reports (ADR 0067): on General the switch and what a report carries in
 * one sentence; on About the last reports this install sent, exactly as sent.
 * On by default in a released build, so the first run says so too (`ErrorReportsLine`). A
 * build from the checkout carries no key and says it sends nothing.
 */
import type { ReportsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

import { Button, Card, ListRow, Section, Switch, errorText, formatDateTime, useToast } from "../../ui";
import { SECTION_TITLE, sectionDomId } from "./model";
import "./settings.css";

/** What a report carries, said once, here and in docs/error-reports.md. */
export const REPORTS_SAY =
  "When something in Ghost breaks, it sends what failed and where in Ghost’s code, with the version and the kind of computer. Never what you said, typed or heard, an app’s words, a password, your name or files.";

/** The first run's sentence, beside the wake-takes line. */
export const REPORTS_FIRST_RUN = "Ghost sends an error report when something in it breaks: what failed and where, never what you say or type.";

export function useReports(): [ReportsView | null, (v: ReportsView) => void] {
  const { client } = useGhost();
  const [view, setView] = useState<ReportsView | null>(null);
  useEffect(() => {
    let on = true;
    client
      .reports()
      .then((v) => on && setView(v))
      .catch(() => undefined);
    return () => {
      on = false;
    };
  }, [client]);
  return [view, setView];
}

export function ErrorReportsSection() {
  const { client } = useGhost();
  const toast = useToast();
  const [view, setView] = useReports();
  if (!view) return null;

  const toggle = async (send: boolean) => {
    try {
      setView(await client.setReportsChoice(send));
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t change it", description: errorText(e) });
    }
  };

  return (
    <Section id={sectionDomId("reports")} title={SECTION_TITLE.reports} className="set-section">
      <Card list>
        {view.available ? (
          <ListRow
            title="Send error reports"
            description={REPORTS_SAY}
            trailing={<Switch checked={view.send} onChange={(on) => void toggle(on)} aria-label="Send error reports" />}
          />
        ) : (
          <ListRow title="Error reports" description="This is a development build: it has no key for error reports and sends none." />
        )}
      </Card>
    </Section>
  );
}

/**
 * The reports this computer sent, exactly as sent: on About beside the diagnostic report (the
 * flow review, 2026-11-14; General keeps the switch). `#/settings/about/sent`.
 */
export function SentReportsSection() {
  const [view] = useReports();
  const [open, setOpen] = useState(false);
  if (!view || view.kept.length === 0) return null;
  return (
    <Section id={sectionDomId("sent")} title={SECTION_TITLE.sent} className="set-section">
      <Card list>
          <ListRow
            title="What was sent"
            description={
              <>
                The last {view.kept.length === 1 ? "report" : `${view.kept.length} reports`} this computer sent, exactly as sent. Kept in Ghost’s data folder.
                {open && (
                  <span className="set-reports">
                    {view.kept.map((k) => (
                      <span className="set-report" key={k.sent_at}>
                        <span className="set-report__head">
                          {formatDateTime(k.sent_at)} · {k.component} · {k.title}
                        </span>
                        <pre className="set-report__body">{JSON.stringify(k.body, null, 2)}</pre>
                      </span>
                    ))}
                  </span>
                )}
              </>
            }
            trailing={
              <Button size="sm" variant="secondary" onClick={() => setOpen(!open)} aria-expanded={open}>
                {open ? "Hide" : "Show"}
              </Button>
            }
          />
        
      </Card>
    </Section>
  );
}

/** The first run's line: one sentence and the switch, at the weight of the wake-takes line. */
export function ErrorReportsLine() {
  const { client } = useGhost();
  const [view, setView] = useReports();
  if (!view?.available) return null;
  const flip = async () => {
    try {
      setView(await client.setReportsChoice(!view.send));
    } catch {
      // Settings › General has the switch too.
    }
  };
  return (
    <p className="onb-try__meta">
      {view.send ? REPORTS_FIRST_RUN : "Error reports are off: Ghost sends nothing when something breaks."}{" "}
      <a href="#/settings/general" onClick={(e) => (e.preventDefault(), void flip())}>
        {view.send ? "Turn off" : "Turn on"}
      </a>
    </p>
  );
}
