/**
 * Owner: watching. What Ghost does and watches while the operator is away. The schedules
 * (ADR 0047, `Schedules.tsx`, from GET /schedules) lead the page; then what Ghost watches
 * (ADR 0035, ADR 0027, ADR 0028), from GET /watches:
 *   #/watching          the watches in watches.json (each read, its schedule, how it decides,
 *                       its last look and counts), the webhooks in hooks.json (how each is
 *                       verified, its events, on or off and why, what it took and refused),
 *                       whether the webhook listener is on, and what is wrong in the files.
 *   #/departments/...   the old address (ADR 0024); it lands here.
 * The folder is the editor: Open folder reveals it. The screen writes one thing there: a
 * watch an agent made (ADR 0052) shows who made it and can be paused, resumed or removed
 * from its row, one entry of watches.json at a time; the operator's own and hooks.json
 * stay the file's.
 */
import type { HookInfo, WatchInfo, WatchingView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { CloudOff, Eye, FolderOpen, MoreHorizontal, Trash2, Webhook } from "lucide-react";
import { Fragment, useEffect, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { navigate, usePath } from "../../app/router";
import { Badge, Button, Card, EmptyState, IconButton, ListRow, Menu, Notice, Page, Section, Skeleton, Switch, agentName, errorText, relativeTime, useConfirm, useLive, useToast } from "../../ui";
import { CONFIG_DIR, RESPONSE, URGENCY, VERIFY, count, everyWords, folderOf, listWords, reasonWords, revealFolder, stepWords, sumOf } from "./model";
import { Schedules } from "./Schedules";
import "./watching.css";

export default function WatchingScreen(_: RouteProps) {
  const path = usePath();
  // #/departments was the screen before ADR 0035: the address lands here, once.
  useEffect(() => {
    if (path.startsWith("/departments")) navigate("/watching", { replace: true });
  }, [path]);

  const { client, waitingVersion } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog: confirmDialog } = useConfirm();
  const [view, setView] = useState<WatchingView | null>(null);
  const [scheduleProblems, setScheduleProblems] = useState<string[]>([]);
  /** The agent's watch a write is going for, and what refused the last one. */
  const [busy, setBusy] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
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

  /** One write to an agent's watch: the row is busy while it goes; a refusal is said above. */
  const act = async (w: WatchInfo, what: string, fn: () => Promise<WatchingView>) => {
    if (busy) return;
    setBusy(w.name);
    setFailed(null);
    try {
      setView(await fn());
    } catch (e) {
      setFailed(`Ghost couldn’t ${what} ${w.name}: ${errorText(e)}`);
    } finally {
      setBusy(null);
    }
  };
  const toggle = (w: WatchInfo, on: boolean) => act(w, on ? "resume" : "pause", () => (on ? client.resumeWatch(w.name) : client.pauseWatch(w.name)));
  const remove = async (w: WatchInfo) => {
    const ok = await confirm({
      title: `Remove ${w.name}?`,
      description: `It stops looking, and its entry leaves watches.json. ${agentName(madeBySlug(w))} can set it up again only with your okay.`,
      confirmLabel: `Remove ${w.name}`,
      tone: "danger",
    });
    if (ok) await act(w, "remove", () => client.removeWatch(w.name));
  };

  return (
    <Page
      width="default"
      title="Watching"
      description="What your agents do on their own, what Ghost keeps an eye on, and the webhooks it takes."
      actions={
        // With nothing watched yet the empty state carries Open folder; one place for it.
        empty ? undefined : (
          <Button variant="secondary" leadingIcon={<FolderOpen />} onClick={() => void openFolder()}>
            Open folder
          </Button>
        )
      }
    >
      {/* Everything that can't run, in one place, before anything else: schedules that won't
          run and the parts of Ghost's folder that are broken. Danger, never amber (amber means
          only “waiting for your okay”). */}
      {(scheduleProblems.length > 0 || (view?.problems.length ?? 0) > 0) && (
        <Notice tone="danger" title={`${count(scheduleProblems.length + (view?.problems.length ?? 0), "thing")} can’t run`} className="wat-notice wat-problems-notice">
          <p className="wat-notice__lead">Ghost goes on without them. Fix each and it picks the change up.</p>
          <ul className="wat-problems">
            {scheduleProblems.map((p) => (
              <li key={p} className="t-small" data-user-content>
                {p}
              </li>
            ))}
            {(view?.problems ?? []).map((p) => (
              <li key={p} className="t-mono t-small" data-user-content>
                {p}
              </li>
            ))}
          </ul>
        </Notice>
      )}
      <Schedules onProblems={setScheduleProblems} />
      <div className="wat-rest">
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
                description={<>Each reads a connected app on its own schedule and keeps only the fields it names. One failure is one item on Today, however often it is seen.</>}
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
                  <>
                    {failed && (
                      <Notice tone="danger" title="That didn’t happen" onDismiss={() => setFailed(null)} className="wat-notice">
                        {failed}
                      </Notice>
                    )}
                    <Card list>
                      {view.watches.map((w) => (
                        <WatchRow
                          key={w.name}
                          w={w}
                          busy={busy === w.name}
                          online={online}
                          reserve={view.watches.some((x) => x.made_by != null)}
                          onToggle={(on) => void toggle(w, on)}
                          onRemove={() => void remove(w)}
                        />
                      ))}
                    </Card>
                  </>
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
            The files: <span className="t-mono">{CONFIG_DIR}/watches.json</span> and <span className="t-mono">hooks.json</span>. They are yours. An agent may add a watch of its own with your okay, marked with who made it; Ghost never changes your own entries or the webhooks.
          </p>
        </>
      )}
      </div>
      {confirmDialog}
    </Page>
  );
}

/** The agent that made a watch, from `agent:<slug>`. */
const madeBySlug = (w: WatchInfo) => (w.made_by ?? "").replace(/^agent:/, "");

interface WatchRowProps {
  w: WatchInfo;
  busy: boolean;
  online: boolean;
  /** Some row has the controls: keep this one's counts in the same column. */
  reserve: boolean;
  onToggle: (on: boolean) => void;
  onRemove: () => void;
}

function WatchRow({ w, busy, online, reserve, onToggle, onRemove }: WatchRowProps) {
  const agents = w.made_by != null;
  const state = w.paused ? (
    <Badge tone="neutral" dot size="sm">
      Paused
    </Badge>
  ) : w.error ? (
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
            {w.acts_through ? `, and ${agentName(w.acts_through)} looks into each new one` : ""}
            {agents ? ` · set up by ${agentName(madeBySlug(w))}` : ""}
          </span>
          {w.error && (
            <span className="wat-error" data-user-content>
              {w.error}
            </span>
          )}
        </span>
      }
      trailing={
        <span className="wat-sched-end">
          <span className="wat-end">
            <span className="wat-end__counts t-num">
              {w.open} open · {w.seen} seen
            </span>
            <span className="wat-end__when">
              {w.paused ? "not looking" : w.last_run ? `looked ${relativeTime(w.last_run)}` : "no look yet"}
              {!w.paused && w.error && w.last_ok ? ` · last worked ${relativeTime(w.last_ok)}` : !w.paused && w.error && w.last_run ? " · never worked" : ""}
            </span>
          </span>
          {!agents && reserve && (
            // The controls' room, unseen and out of reach, so every row's counts line up.
            <span className="wat-hold" aria-hidden inert>
              <Switch checked={false} onChange={() => {}} size="sm" aria-label="" />
              <IconButton label="" icon={<MoreHorizontal />} variant="ghost" size="sm" />
            </span>
          )}
          {agents && (
            <>
              <Switch checked={!w.paused} onChange={onToggle} disabled={busy || !online} size="sm" aria-label={`Keep ${w.name} looking`} />
              <Menu
                align="end"
                trigger={<IconButton label={`More for ${w.name}`} icon={<MoreHorizontal />} variant="ghost" size="sm" />}
                items={[{ label: "Remove…", icon: <Trash2 />, danger: true, onSelect: onRemove, disabled: busy || !online }]}
              />
            </>
          )}
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
