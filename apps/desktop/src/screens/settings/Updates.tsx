/**
 * Owner: settings. Updates (ADR 0066), in General: this version, whether a new one is there
 * and what's new in it (the release notes, as plain text), "Check now", the "Check for
 * updates" switch with what the check sends, and the one button that installs: "Update and
 * restart". The button is the operator's alone.
 */
import { useState } from "react";

import { DESKTOP_VERSION } from "../../app/version";
import { type UpdateView, availableLine, useUpdate, cancelUpdate, checkForUpdate, installUpdate, plainNotes, setUpdateCheck } from "../../app/update";
import { Badge, Button, Card, ListRow, Notice, Progress, Section, Switch, errorText } from "../../ui";
import { SECTION_TITLE, sectionDomId } from "./model";
import "./settings.css";

const CHECK_SAYS =
  "When Ghost starts and once a day: one request to GitHub for the latest release’s update file. It carries nothing about you or this computer beyond what any web request does.";

function ago(secs: number | null): string {
  if (!secs) return "Not checked yet.";
  const m = Math.round((Date.now() / 1000 - secs) / 60);
  if (m < 1) return "Checked just now.";
  if (m < 60) return `Checked ${m} minute${m === 1 ? "" : "s"} ago.`;
  const h = Math.round(m / 60);
  return h < 48 ? `Checked ${h} hour${h === 1 ? "" : "s"} ago.` : `Checked ${Math.round(h / 24)} days ago.`;
}

const mb = (n: number) => `${Math.round(n / 1e6)} MB`;

export function UpdateRows({ view, onView }: { view: UpdateView | null; onView: (v: UpdateView) => void }) {
  const [error, setError] = useState<string | null>(null);
  const act = (f: () => Promise<UpdateView | void>) => async () => {
    setError(null);
    try {
      const v = await f();
      if (v) onView(v);
    } catch (e) {
      setError(errorText(e));
    }
  };
  if (!view) return null;
  if (view.unavailable) {
    return (
      <Card list>
        <ListRow title="Updates" description={view.unavailable} />
      </Card>
    );
  }
  const p = view.phase;
  const busy = p.state === "downloading" || p.state === "installing" || p.state === "restarting" || p.state === "waiting";
  const offer = view.available;
  return (
    <>
      {error && (
        <Notice tone="danger" title="That didn’t work" onDismiss={() => setError(null)}>
          {error}
        </Notice>
      )}
      {/* A failed install is the operator's to see; a failed background check is a quiet line below. */}
      {p.state === "failed" && offer && (
        <Notice tone="danger" title="The update didn’t finish">
          {p.error}
        </Notice>
      )}
      <Card list>
        {offer ? (
          <ListRow
            title={availableLine(view)}
            description={
              p.state === "downloading" ? (
                <>
                  Downloading {p.total ? `${mb(p.done)} of ${mb(p.total)}` : mb(p.done)}, then checking its signature.
                  <Progress value={p.total ? (p.done / p.total) * 100 : undefined} label="Downloading the update" />
                </>
              ) : p.state === "waiting" ? (
                p.reason
              ) : p.state === "installing" ? (
                view.installer === "deb" ? "Installing. Ubuntu asks for your password once: the .deb is a system package." : "Installing."
              ) : p.state === "restarting" ? (
                "Restarting Ghost and the app."
              ) : (
                `You have ${view.current}. Ghost restarts with the new version; your settings, agents and memory stay as they are.`
              )
            }
            trailing={
              p.state === "waiting" ? (
                <Button size="sm" variant="secondary" onClick={act(cancelUpdate)}>
                  Not now
                </Button>
              ) : (
                <Button size="sm" loading={busy} disabled={busy} onClick={act(installUpdate)}>
                  {p.state === "failed" ? "Try again" : "Update and restart"}
                </Button>
              )
            }
          />
        ) : (
          <ListRow
            title={`Ghost ${view.current}`}
            description={p.state === "checking" ? "Checking…" : p.state === "failed" ? `${p.error} Ghost tries again tomorrow, or now with Check now.` : view.checked_at ? `Up to date. ${ago(view.checked_at)}` : ago(null)}
            trailing={
              <Button size="sm" variant="secondary" loading={p.state === "checking"} onClick={act(checkForUpdate)}>
                Check now
              </Button>
            }
          />
        )}
        {offer?.notes && (
          <ListRow
            title="What’s new"
            description={<span className="set-notes">{plainNotes(offer.notes)}</span>}
          />
        )}
        <ListRow
          title="Check for updates"
          description={CHECK_SAYS}
          trailing={<Switch checked={view.check} onChange={(on) => void act(() => setUpdateCheck(on))()} aria-label="Check for updates" />}
        />
        {offer && (
          <ListRow
            title="Installed as"
            description={view.installer === "appimage" ? "An AppImage: the update replaces it where it is." : "A .deb package: the update installs the new package, asking for your password once."}
            trailing={<Badge>{view.installer === "appimage" ? "AppImage" : ".deb"}</Badge>}
          />
        )}
      </Card>
    </>
  );
}

/** General's Updates section; nothing outside the desktop app, where there is no updater. */
export function UpdatesSection() {
  const [view, setView] = useUpdate(DESKTOP_VERSION);
  if (!view) return null;
  return (
    <Section id={sectionDomId("updates")} title={SECTION_TITLE.updates} className="set-section">
      <UpdateRows view={view} onView={setView} />
    </Section>
  );
}
