/**
 * Owner: settings. “Save a diagnostic report” (ADR 0072), on About: one file in your
 * Downloads folder that you can hand to a developer. The daemon builds it (`POST /diagnose`)
 * from what Ghost already knows; nothing is uploaded anywhere. Before saving, the person sees
 * what the file will hold, part by part, and its size (`POST /diagnose/preview`). Three boxes,
 * unticked, add their own words or voice, each with a plain warning. docs/diagnostic-report.md
 * lists every field.
 */
import type { DiagnosePreview, DiagnoseRequest } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { FileDown, TriangleAlert } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import { useFloating } from "../../app/floating";
import { useUpdate } from "../../app/update";
import { DESKTOP_VERSION, windowLine } from "../../app/version";
import { Button, Card, Checkbox, Dialog, ListRow, Notice, Section, Skeleton, errorText, useToast } from "../../ui";
import { SECTION_TITLE, sectionDomId } from "./model";

export const BOXES: { key: "transcripts" | "debug_log" | "wake_recordings"; label: string; warning: string }[] = [
  {
    key: "transcripts",
    label: "Add recent transcripts",
    warning: "Your last turns with Ghost, word for word: what you said and typed, and its answers. Anyone you give the file to can read them.",
  },
  {
    key: "debug_log",
    label: "Add the debug log",
    warning: "Only while debug mode is on: everything Ghost heard in that time, word for word, even what wasn’t meant for it.",
  },
  {
    key: "wake_recordings",
    label: "Add kept wake recordings",
    warning: "Up to eight recordings of your voice saying Ghost’s name, with a few seconds before it. Anyone with the file can play them.",
  },
];

export const size = (n: number) => (n < 1024 ? `${n} bytes` : n < 1024 * 1024 ? `${Math.round(n / 1024)} KB` : `${(n / 1024 / 1024).toFixed(1)} MB`);

/** The shell's own facts the daemon doesn't know, each one short line. */
function useShellFacts(): Record<string, string> {
  const { status } = useFloating();
  const [update] = useUpdate(DESKTOP_VERSION);
  const facts: Record<string, string> = { window: windowLine(true) };
  if (status) facts.floating = status.support.kind === "no" ? `can't float: ${status.support.why}` : `can float (${status.support.kind}); ${status.settings.on ? "on" : "off"}`;
  if (update) facts.updates = `${update.check ? "checks" : "doesn't check"}; ${update.available ? `${update.available.version} available` : "none waiting"}`;
  return facts;
}

export function DiagnosticReport({ openOnLoad = false }: { openOnLoad?: boolean }) {
  const [open, setOpen] = useState(openOnLoad);
  return (
    <Section
      id={sectionDomId("diagnostic")}
      title={SECTION_TITLE.diagnostic}
      description="One file you can hand to whoever is helping you. Ghost writes it to your Downloads folder and sends nothing anywhere."
      className="set-section"
    >
      <Card list>
        <ListRow
          title="Save a diagnostic report"
          description="This version, this computer, sound, the model’s names and timings, recent turns’ timings, wake scores, error reports and Ghost’s own log, scrubbed. Never a password, a home path, an app’s content or what you said, unless you add it."
          trailing={
            <Button size="sm" variant="secondary" leadingIcon={<FileDown />} onClick={() => setOpen(true)}>
              Save a report…
            </Button>
          }
        />
      </Card>
      {open && <ReportDialog onClose={() => setOpen(false)} />}
    </Section>
  );
}

function ReportDialog({ onClose }: { onClose: () => void }) {
  const { client } = useGhost();
  const toast = useToast();
  const shell = useShellFacts();
  const [boxes, setBoxes] = useState({ transcripts: false, debug_log: false, wake_recordings: false });
  const [preview, setPreview] = useState<DiagnosePreview | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const shellKey = JSON.stringify(shell);
  const request = useCallback((): DiagnoseRequest => ({ ...boxes, folder: null, shell: JSON.parse(shellKey) }), [boxes, shellKey]);

  useEffect(() => {
    let live = true;
    client
      .diagnosePreview(request())
      .then((p) => {
        if (!live) return;
        setPreview(p);
        setError(null);
      })
      .catch((e) => live && setError(errorText(e)));
    return () => {
      live = false;
    };
  }, [client, request]);

  const save = async () => {
    setBusy(true);
    try {
      const r = await client.diagnose(request());
      toast({ title: "Report saved", description: `${r.path} (${size(r.bytes)}). Nothing was sent anywhere.`, tone: "ok" });
      onClose();
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const personal = BOXES.filter((b) => boxes[b.key]);
  return (
    <Dialog
      open
      onClose={onClose}
      size="md"
      dismissible={!busy}
      title="Save a diagnostic report"
      description={preview ? `It will be saved as ${preview.path}, ${size(preview.bytes)}. Ghost uploads nothing; you choose who gets it.` : "Working out what the file will hold…"}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" onClick={() => void save()} loading={busy} disabled={!preview}>
            Save report
          </Button>
        </>
      }
    >
      {error && <Notice tone="danger" title="Couldn’t build the report">{error}</Notice>}
      <div className="set-diag">
        <p className="set-diag__lead">What the file holds:</p>
        {!preview ? (
          <Skeleton height={120} />
        ) : (
          <ul className="set-diag__parts" role="list">
            {preview.parts.map((p) => (
              <li key={p.key} className={p.personal ? "is-personal" : undefined}>
                <span className="set-diag__title">
                  {p.personal && <TriangleAlert size={14} aria-label="Your own words or voice" />}
                  {p.title}
                </span>
                <span className="set-diag__summary">{p.summary}</span>
                <span className="set-diag__size t-num">{size(p.bytes)}</span>
              </li>
            ))}
          </ul>
        )}
        <p className="set-diag__lead">Add, if whoever is helping you needs it:</p>
        {BOXES.map((b) => {
          const off = b.key === "debug_log" && preview !== null && !preview.debug_log_available;
          const none = b.key === "wake_recordings" && preview !== null && preview.wake_recordings === 0;
          return (
            <div key={b.key} className="set-diag__box">
              <Checkbox
                id={`set-diag-${b.key}`}
                checked={boxes[b.key]}
                disabled={off || none || busy}
                onChange={(on: boolean) => setBoxes((x) => ({ ...x, [b.key]: on }))}
                label={b.label}
              />
              <p className="set-diag__warning">{off ? "Debug mode isn’t on, so there is no debug log to add. Turn it on in Settings › Developer, do what goes wrong, then save the report." : none ? "No wake recordings are kept." : b.warning}</p>
            </div>
          );
        })}
        {personal.length > 0 && (
          <Notice tone="warn" title="This file will hold your own words or voice">
            {personal.map((b) => b.label.replace(/^Add /, "")).join(", ")}. Give it only to someone you trust with them, and delete it when they’re done.
          </Notice>
        )}
      </div>
    </Dialog>
  );
}
