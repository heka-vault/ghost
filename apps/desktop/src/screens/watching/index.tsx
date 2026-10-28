/**
 * Owner: watching. What Ghost watches (ADR 0035, ADR 0027, ADR 0028), from GET /watches:
 *   #/watching          the watches in watches.json (each read, its schedule, how it decides,
 *                       its last look and counts), the webhooks in hooks.json (how each is
 *                       verified, its events, on or off and why, what it took and refused),
 *                       whether the webhook listener is on, and what is wrong in the files.
 *   #/departments/...   the old address (ADR 0024); it lands here.
 * The folder is the editor: Open folder reveals it; the screen never writes either file.
 */
import type { HookInfo, WatchInfo, WatchingView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { CloudOff, Eye, FolderOpen, Webhook } from "lucide-react";
import { Fragment, useEffect, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { navigate, usePath } from "../../app/router";
import { Badge, Button, Card, EmptyState, ListRow, Notice, Page, Section, Skeleton, errorText, relativeTime, useToast } from "../../ui";
import { CONFIG_DIR, RESPONSE, URGENCY, VERIFY, count, everyWords, folderOf, listWords, reasonWords, revealFolder, stepWords, sumOf } from "./model";
import "./watching.css";

export default function WatchingScreen(_: RouteProps) {
  const path = usePath();
  // #/departments was the screen before ADR 0035: the address lands here, once.
  useEffect(() => {
    if (path.startsWith("/departments")) navigate("/watching", { replace: true });
  }, [path]);

  const { client, waitingVersion } = useGhost();
  const toast = useToast();
  const [view, setView] = useState<WatchingView | null>(null);
  /** Ghost's folder, from where its AGENTS.md is: the one path the daemon reports for it. */
  const [folder, setFolder] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let on = true;
    client
      .watches()
      .then((v) => on && (setView(v), setLoadError(null)))
      .catch((e) => on && setLoadError(errorText(e)));
    client
      .rules()
      .then((r) => on && setFolder(folderOf(r.path)))
      .catch(() => {});
    return () => {
      on = false;
    };
    // A new item means a watch looked or a webhook landed: read the counts again.
  }, [client, waitingVersion, attempt]);

  const openFolder = async () => {
    const r = folder ? await revealFolder(folder) : { ok: false as const, path: CONFIG_DIR };
    if (!r.ok) toast({ title: "Ghost’s folder", description: `${r.path} (open it in your file manager; this build can’t open folders itself).` });
  };

  const empty = view != null && view.watches.length === 0 && view.hooks.length === 0;

  return (
    <Page
      width="default"
      title="Watching"
      description={<>What Ghost keeps an eye on and the webhooks it takes, from watches.json and hooks.json in its folder. Each look is a plain read and a check; a watch asks Jev or the model only where it says so.</>}
      actions={
        // With nothing watched yet the empty state carries Open folder; one place for it.
        empty ? undefined : (
          <Button variant="secondary" leadingIcon={<FolderOpen />} onClick={() => void openFolder()}>
            Open folder
          </Button>
        )
      }
    >
      {loadError && view != null && (
        <Notice tone="danger" title="Couldn’t refresh what Ghost watches" onDismiss={() => setLoadError(null)} className="wat-notice">
          {loadError} This is the list as it was.
        </Notice>
      )}
      {view == null && loadError ? (
        <Card>
          <EmptyState
            icon={<CloudOff />}
            title="Couldn’t load what Ghost watches"
            description="Ghost didn’t send it. Try again in a moment."
            actions={<Button onClick={() => setAttempt((n) => n + 1)}>Try again</Button>}
          />
        </Card>
      ) : view == null ? (
        <div className="stack gap-6" aria-busy="true" aria-label="Loading">
          {[0, 1].map((s) => (
            <div key={s} className="stack gap-3">
              <Skeleton width={s ? 88 : 72} height={16} />
              <Card list>
                {[0, 1].map((i) => (
                  <div key={i} className="wat-skel-row">
                    <Skeleton width={20} height={20} radius="var(--radius-sm)" />
                    <span className="stack gap-2 grow">
                      <Skeleton width={i ? 96 : 64} height={12} />
                      <Skeleton width={i ? 260 : 320} height={10} />
                    </span>
                  </div>
                ))}
              </Card>
            </div>
          ))}
        </div>
      ) : (
        <>
          {view.problems.length > 0 && (
            <Notice tone="warn" title={`${count(view.problems.length, "problem")} in Ghost’s folder`} className="wat-notice">
              <p className="wat-notice__lead">Ghost runs without the part that is broken. Fix the file and it picks the change up.</p>
              <ul className="wat-problems">
                {view.problems.map((p) => (
                  <li key={p} className="t-mono t-small" data-user-content>
                    {p}
                  </li>
                ))}
              </ul>
            </Notice>
          )}

          {empty ? (
            <Card>
              <EmptyState
                icon={<Eye />}
                title="Ghost isn’t watching anything yet"
                description={
                  <>
                    Add a watch to <span className="t-mono">{CONFIG_DIR}/watches.json</span>: a connected app, one of its reads and how often, and what to keep from it. Webhooks go in{" "}
                    <span className="t-mono">hooks.json</span> beside it. Both are set out in <span className="t-mono">docs/folder.md</span>.
                  </>
                }
                actions={
                  <Button leadingIcon={<FolderOpen />} onClick={() => void openFolder()}>
                    Open folder
                  </Button>
                }
              />
            </Card>
          ) : (
            <>
              <Section
                id="wat-watches"
                title="Watches"
                description={<>Each reads a connected app on its own schedule and keeps only the fields it names. One failure is one item on What needs you, however often it is seen.</>}
              >
                {view.watches.length === 0 ? (
                  <Card>
                    <EmptyState
                      size="compact"
                      icon={<Eye />}
                      title="No watches"
                      description={
                        <>
                          Add one to <span className="t-mono">watches.json</span> in Ghost’s folder, keyed by its name; <span className="t-mono">docs/folder.md</span> shows the shape.
                        </>
                      }
                    />
                  </Card>
                ) : (
                  <Card list>
                    {view.watches.map((w) => (
                      <WatchRow key={w.name} w={w} />
                    ))}
                  </Card>
                )}
              </Section>

              <Section
                id="wat-hooks"
                title="Webhooks"
                description={
                  <>
                    {view.listening.length > 0 ? (
                      <>
                        Ghost takes them on <span className="t-mono">{listWords(view.listening)}</span>, its own addresses. A delivery is checked for its signature before Ghost reads it.
                      </>
                    ) : (
                      <>
                        The listener is off, so none of these can arrive. Set <span className="t-mono">GHOST_HOOKS_LISTEN</span> in Ghost’s settings to turn it on.
                      </>
                    )}
                  </>
                }
                actions={
                  <Badge tone={view.listening.length ? "ok" : "neutral"} dot size="sm">
                    {view.listening.length ? "Listening" : "Listener off"}
                  </Badge>
                }
              >
                {view.hooks.length === 0 ? (
                  <Card>
                    <EmptyState
                      size="compact"
                      icon={<Webhook />}
                      title="No webhooks"
                      description={
                        <>
                          Add one to <span className="t-mono">hooks.json</span> in Ghost’s folder, with recorded deliveries in <span className="t-mono">hooks/&lt;name&gt;/</span>; <span className="t-mono">docs/folder.md</span> shows the shape.
                        </>
                      }
                    />
                  </Card>
                ) : (
                  <Card list>
                    {view.hooks.map((h) => (
                      <HookRow key={h.name} h={h} />
                    ))}
                  </Card>
                )}
              </Section>
            </>
          )}

          <p className="wat-files t-small t-3">
            The files: <span className="t-mono">{CONFIG_DIR}/watches.json</span> and <span className="t-mono">hooks.json</span>. They are yours; Ghost reads them and never writes them.
          </p>
        </>
      )}
    </Page>
  );
}

function WatchRow({ w }: { w: WatchInfo }) {
  const state = w.error ? (
    <Badge tone="danger" dot size="sm">
      Can’t read
    </Badge>
  ) : !w.last_run ? (
    <Badge tone="neutral" dot size="sm">
      Not run yet
    </Badge>
  ) : (
    <Badge tone="ok" dot size="sm">
      Looking
    </Badge>
  );
  return (
    <ListRow
      align="start"
      leading={
        <span className="wat-icon">
          <Eye aria-hidden />
        </span>
      }
      title={
        <span className="wat-title">
          <span className="t-mono">{w.name}</span>
          {state}
        </span>
      }
      description={
        <span className="wat-desc">
          <span>
            Reads <span className="t-mono">{`${w.app}.${w.read}`}</span> {everyWords(w.every_secs)}, decides with {stepWords(w.steps)}.
          </span>
          <span className="t-3">
            {RESPONSE[w.response]}
            {w.response === "tell" ? ` ${URGENCY[w.urgency]}` : ""}
          </span>
          {w.error && (
            <span className="wat-error" data-user-content>
              {w.error}
            </span>
          )}
        </span>
      }
      trailing={
        <span className="wat-end">
          <span className="wat-end__counts t-num">
            {w.open} open · {w.seen} seen
          </span>
          <span className="wat-end__when">
            {w.last_run ? `looked ${relativeTime(w.last_run)}` : "no look yet"}
            {w.error && w.last_ok ? ` · last worked ${relativeTime(w.last_ok)}` : w.error && w.last_run ? " · never worked" : ""}
          </span>
        </span>
      }
    />
  );
}

function HookRow({ h }: { h: HookInfo }) {
  const taken = sumOf(h.accepted);
  const refused = sumOf(h.rejected);
  const accepted = Object.entries(h.accepted)
    .filter(([, n]) => n > 0)
    .map(([k, n]) => (k === "item" ? count(n, "item") : `${n} ${k}`))
    .join(", ");
  return (
    <ListRow
      align="start"
      leading={
        <span className="wat-icon">
          <Webhook aria-hidden />
        </span>
      }
      title={
        <span className="wat-title">
          <span className="t-mono">{h.name}</span>
          <Badge tone={h.on ? "ok" : "neutral"} dot size="sm">
            {h.on ? "On" : "Off"}
          </Badge>
        </span>
      }
      description={
        <span className="wat-desc">
          <span>
            <span className="t-mono">POST {h.path}</span>, checked by {VERIFY[h.verify]}. {h.events.length ? (
              <>
                Counts{" "}
                {h.events.map((ev, i) => (
                  <Fragment key={ev}>
                    {i > 0 && (i === h.events.length - 1 ? " and " : ", ")}
                    <span className="t-mono">{ev}</span>
                  </Fragment>
                ))}
                .
              </>
            ) : (
              "No events declared."
            )}
          </span>
          {!h.on && <span className="wat-off">Off: {h.off_reason ?? "the listener is off"}</span>}
          {refused > 0 && <span className="t-3">Refused: {reasonWords(h.rejected)}</span>}
        </span>
      }
      trailing={
        <span className="wat-end">
          <span className="wat-end__counts t-num">
            {taken} taken · {refused} refused
          </span>
          <span className="wat-end__when">
            {h.last ? `last ${relativeTime(h.last)}` : "nothing yet"}
            {accepted ? ` · ${accepted}` : ""}
          </span>
        </span>
      }
    />
  );
}
