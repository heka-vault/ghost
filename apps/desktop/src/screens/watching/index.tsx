/**
 * Owner: watching. On its own (`#/on-its-own`; `#/watching` and `#/departments/…` redirect):
 * everything Ghost and its agents do without being asked, as one list of plain sentences (the
 * owner's simplification, 2026-11-14). From the top:
 *   Running now   each run going this moment, with Stop (GET /runs, the `run` event)
 *   Ask Ghost     the way to make one: a sentence to Ghost (`AskGhost.tsx`)
 *   The list      schedules (ADR 0047), watches and watches that act (ADR 0028, ADR 0052) and
 *                 webhooks (ADR 0027), each a sentence (`sentence.ts`) with its state; a schedule
 *                 has its switch and Run now, Stop, What it did…, Edit…, Remove…; a watch an agent
 *                 made has its switch and Remove…; every watch and webhook has Details…
 *   By hand       the forms as they were: Add schedule, Add watch, and webhooks (`hooks.json`,
 *                 its listener and its files), one disclosure down
 * The words schedule, watch, webhook and run are in the details and the by-hand forms, never what
 * a newcomer chooses between. The window writes `watches.json` only as before: pause, resume or
 * remove a watch an agent made, one entry at a time; the operator's own and `hooks.json` stay the
 * file's.
 */
import type { ConversationView, HookInfo, ScheduleInfo, ScheduleRunInfo, SchedulesView, WatchInfo, WatchingView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { CalendarClock, CloudOff, Eye, FolderOpen, History, Info, MoreHorizontal, Pencil, Play, Plus, Repeat, Square, Trash2, Webhook } from "lucide-react";
import { Fragment, useCallback, useEffect, useState } from "react";

import { Link } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { Badge, Button, Card, cx, Dialog, Disclosure, EmptyState, IconButton, LineGlyph, ListRow, Menu, Notice, Page, Section, Skeleton, Switch, agentName, errorText, relativeTime, useConfirm, useLive, useToast } from "../../ui";
import { toolTitle } from "../settings/tools";
import { AddWatchDialog } from "./AddWatch";
import { AskGhost } from "./AskGhost";
import { CONFIG_DIR, RESPONSE, URGENCY, VERIFY, count, everyWords, folderOf, listWords, madeBy, reasonWords, revealFolder, runErrorWords, stepWords, sumOf, untilWords } from "./model";
import { OnItsOwnList } from "./List";
import { type Problem, problemOf } from "./problems";
import { RunsDialog, ScheduleDialog } from "./Schedules";
import { useAppTitles } from "./Runs";
import { type Doing, countWords, doingsOf, goingNow, goingWords, ownersOf, ownsDoing, placeProblems, sentenceOf } from "./sentence";
import "./watching.css";

/** How often the list is read again while the page is open: next and last runs move. */
const REFRESH_MS = 15_000;

export default function OnItsOwnScreen(_: RouteProps) {
  const { client, waitingVersion, runsVersion } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog: confirmDialog } = useConfirm();
  const appTitle = useAppTitles();
  const [schedules, setSchedules] = useState<SchedulesView | null>(null);
  const [watching, setWatching] = useState<WatchingView | null>(null);
  const [runs, setRuns] = useState<ScheduleRunInfo[]>([]);
  const [agents, setAgents] = useState<ConversationView[]>([]);
  const [folder, setFolder] = useState<string | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [busy, setBusy] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  /** The agent the page is showing, or everyone. */
  const [only, setOnly] = useState<string | null>(null);
  // The dialogs: the by-hand forms, a row's history and a row's details.
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<ScheduleInfo | null>(null);
  const [addingWatch, setAddingWatch] = useState(false);
  const [history, setHistory] = useState<{ title: string; load: () => Promise<ScheduleRunInfo[]> } | null>(null);
  const [details, setDetails] = useState<Doing | null>(null);
  const [problem, setProblem] = useState<Problem | null>(null);

  const reload = useCallback(() => setAttempt((n) => n + 1), []);
  useEffect(() => {
    const id = setInterval(reload, REFRESH_MS);
    return () => clearInterval(id);
  }, [reload]);

  useEffect(() => {
    let on = true;
    client
      .schedules()
      .then((v) => on && (setSchedules(v), setLoadError(null)))
      .catch((e) => on && setLoadError(errorText(e)));
    client
      .watches()
      .then((v) => on && setWatching(v))
      .catch((e) => on && setLoadError(errorText(e)));
    client
      .runs(30)
      .then((r) => on && setRuns(r.runs))
      .catch(() => {});
    return () => {
      on = false;
    };
    // A run's answer lands on Today and a run starts or ends: read it all again.
  }, [client, waitingVersion, runsVersion, attempt]);

  useEffect(() => {
    let on = true;
    client
      .conversations(false)
      .then((l) => on && setAgents(l))
      .catch(() => {});
    client
      .rules()
      .then((r) => on && setFolder(folderOf(r.path)))
      .catch(() => {});
    return () => {
      on = false;
    };
  }, [client]);

  const nameOf = useCallback((slug: string) => agentName(slug, agents.find((a) => a.slug === slug)?.name), [agents]);
  const loaded = schedules != null && watching != null;
  const all = loaded ? doingsOf(schedules.schedules, watching.watches, watching.hooks) : [];
  const owners = ownersOf(all);
  const shown = all.filter((d) => !only || ownsDoing(d, only));
  const going = goingNow(runs);
  const ctx = { zone: schedules?.time_zone ?? "", agentName: nameOf, appTitle };
  // What can't run goes to its row, or is a row of its own (the flow review, 2026-11-14).
  const placed = placeProblems(all, [...(schedules?.problems ?? []), ...(watching?.problems ?? [])].map((raw) => problemOf(raw, { appTitle, agentName: nameOf })));

  /** One write: the row is busy while it goes; a refusal is said above the list. */
  const act = async (key: string, what: string, fn: () => Promise<void>) => {
    if (busy) return;
    setBusy(key);
    setFailed(null);
    try {
      await fn();
    } catch (e) {
      setFailed(`Ghost couldn’t ${what}: ${errorText(e)}`);
    } finally {
      setBusy(null);
    }
  };
  const putSchedule = (s: ScheduleInfo) => setSchedules((v) => (v ? { ...v, schedules: v.schedules.map((x) => (x.id === s.id ? s : x)) } : v));

  const toggle = (d: Doing, on: boolean) =>
    d.kind === "schedule"
      ? act(d.key, `${on ? "resume" : "pause"} ${d.s.name}`, async () => putSchedule(on ? await client.resumeSchedule(d.s.id) : await client.pauseSchedule(d.s.id)))
      : d.kind === "watch"
        ? act(d.key, `${on ? "resume" : "pause"} ${d.w.name}`, async () => setWatching(on ? await client.resumeWatch(d.w.name) : await client.pauseWatch(d.w.name)))
        : Promise.resolve();
  const runNow = (s: ScheduleInfo) =>
    act(`schedule:${s.id}`, `run ${s.name}`, async () => {
      await client.runSchedule(s.id);
      putSchedule({ ...s, running: true });
      reload();
      toast({ title: "Started", description: `${nameOf(s.agent)} is on it. The answer goes to Today.` });
    });
  const stopRun = (r: ScheduleRunInfo) =>
    act(`run:${r.id}`, `stop ${r.name}`, async () => {
      await client.cancelRun(r.id);
      reload();
      toast({ title: "Stopped", description: `${nameOf(r.agent)} stops where it is. What it already changed is on Today.` });
    });
  /** Stop a schedule's run from its row: the one going or waiting. */
  const stopSchedule = (s: ScheduleInfo) =>
    act(`schedule:${s.id}`, `stop ${s.name}`, async () => {
      const { runs: theirs } = await client.scheduleRuns(s.id, 5);
      const r = theirs.find((x) => x.status === "running" || x.status === "queued");
      if (!r) throw new Error("no run of it is going now");
      await client.cancelRun(r.id);
      putSchedule({ ...s, running: false });
      reload();
    });
  const remove = async (d: Doing) => {
    if (d.kind === "schedule") {
      const ok = await confirm({ title: `Remove ${d.s.name}?`, description: "It stops, and what its runs said goes with it. What it already put on Today stays.", confirmLabel: `Remove ${d.s.name}`, tone: "danger" });
      if (ok)
        await act(d.key, `remove ${d.s.name}`, async () => {
          await client.removeSchedule(d.s.id);
          setSchedules((v) => (v ? { ...v, schedules: v.schedules.filter((x) => x.id !== d.s.id) } : v));
        });
    } else if (d.kind === "watch") {
      const ok = await confirm({
        title: `Remove ${d.w.name}?`,
        description: `It stops looking, and its entry leaves watches.json. ${nameOf(madeBySlug(d.w))} can set it up again only with your yes.`,
        confirmLabel: `Remove ${d.w.name}`,
        tone: "danger",
      });
      if (ok) await act(d.key, `remove ${d.w.name}`, async () => setWatching(await client.removeWatch(d.w.name)));
    }
  };
  const openFolder = async () => {
    const r = folder ? await revealFolder(folder) : { ok: false as const, path: CONFIG_DIR };
    if (!r.ok) toast({ title: "Ghost’s folder", description: `${r.path} (open it in your file manager; this build can’t open folders itself).` });
  };

  return (
    <Page width="default" title="On its own" description="What Ghost and your agents do without being asked.">
      <AskGhost onReply={reload} />

      {going.length > 0 && (
        <Section id="wat-now" title="Running now" description="Stop one and it stops where it is. What it already changed stays, and is on Today.">
          <Card list className="wat-now">
            {going.map((r) => (
              <ListRow
                key={r.id}
                align="center"
                leading={<LineGlyph slug={r.agent} name={nameOf(r.agent)} size="lg" />}
                title={
                  <span className="wat-title">
                    <span>{goingWords(r, nameOf)}</span>
                    <Badge tone="info" dot size="sm">
                      {r.status === "queued" ? "Starting" : "Running"}
                    </Badge>
                  </span>
                }
                description={<span className="t-3 t-num">{r.started_at ? `started ${relativeTime(r.started_at)}` : "about to start"}{r.tool_calls ? ` · ${count(r.tool_calls, "call")} so far` : ""}</span>}
                trailing={
                  <Button variant="secondary" size="sm" leadingIcon={<Square />} loading={busy === `run:${r.id}`} disabled={!online} onClick={() => void stopRun(r)} aria-label={`Stop ${goingWords(r, nameOf)}`}>
                    Stop
                  </Button>
                }
              />
            ))}
          </Card>
        </Section>
      )}

      <Section
        id="wat-list"
        title={loaded ? countWords(all.length + placed.rows.length) : "Everything it does"}
        className="wat-list"
      >
        {owners.length > 1 && (
          <div className="wat-owners" role="radiogroup" aria-label="Whose">
            {[null, ...owners].map((slug) => (
              <button key={slug ?? "all"} type="button" role="radio" aria-checked={only === slug} className={cx("wat-owner", only === slug && "is-on")} onClick={() => setOnly(slug)}>
                {slug ? <LineGlyph slug={slug} name={nameOf(slug)} size="sm" /> : null}
                {slug ? nameOf(slug) : "Everyone"}
              </button>
            ))}
          </div>
        )}
        {failed && (
          <Notice tone="danger" title="That didn’t happen" onDismiss={() => setFailed(null)} className="wat-notice">
            {failed}
          </Notice>
        )}
        {loadError && loaded && (
          <Notice tone="danger" title="Couldn’t refresh the list" onDismiss={() => setLoadError(null)} className="wat-notice">
            {loadError} This is the list as it was.
          </Notice>
        )}
        {!loaded && loadError ? (
          <Card>
            <EmptyState icon={<CloudOff />} title="Couldn’t load what Ghost does on its own" description="Ghost didn’t send it. Try again in a moment." actions={<Button onClick={reload}>Try again</Button>} />
          </Card>
        ) : !loaded ? (
          <Card list aria-busy="true" aria-label="Loading">
            {[0, 1, 2].map((i) => (
              <div key={i} className="wat-skel-row">
                <Skeleton width={28} height={28} radius="var(--radius-md)" />
                <span className="stack gap-2 grow">
                  <Skeleton width={i ? 300 : 360} height={12} />
                  <Skeleton width={i ? 180 : 220} height={10} />
                </span>
              </div>
            ))}
          </Card>
        ) : shown.length === 0 && placed.rows.length === 0 ? (
          <Card>
            <EmptyState
              icon={<Repeat />}
              title={only ? `${nameOf(only)} does nothing on its own yet` : "Ghost does nothing on its own yet"}
              description={<>Ask it above, in your own words: “every weekday at nine, check the build and tell me if it’s red”, or “tell me when a new issue is opened in the app repo”. It shows here once it is set up.</>}
            />
          </Card>
        ) : (
          <OnItsOwnList
            rows={shown.map((d) => ({ d, s: sentenceOf(d, ctx), problem: placed.byKey.get(d.key) ?? null }))}
            problems={only && only !== "ghost" ? [] : placed.rows}
            busyKey={busy}
            online={online}
            on={{
              toggle: (d, on) => void toggle(d, on),
              run: (d) => d.kind === "schedule" && void runNow(d.s),
              stop: (d) => d.kind === "schedule" && void stopSchedule(d.s),
              edit: (d) => d.kind === "schedule" && setEditing(d.s),
              history: (d) =>
                setHistory(
                  d.kind === "schedule"
                    ? { title: d.s.name, load: () => client.scheduleRuns(d.s.id, 10).then((r) => r.runs) }
                    : d.kind === "watch"
                      ? { title: d.w.name, load: () => client.runs(60).then((r) => r.runs.filter((x) => x.source === `watch:${d.w.name}`).slice(0, 10)) }
                      : null,
                ),
              details: (d) => setDetails(d),
              remove: (d) => void remove(d),
              problemDetails: (p) => setProblem(p),
            }}
          />
        )}
      </Section>

      <div id="wat-hand" className="wat-hand">
        <Disclosure summary="Set it up by hand" defaultOpen={new URLSearchParams(window.location.search).get("hand") === "1"}>
          <p className="t-small t-2 wat-hand__lead">The forms, for when you know exactly what you want. Anything you set up here is on the list above like the rest.</p>
          <Card list>
            <ListRow
              align="center"
              leading={<span className="wat-icon"><CalendarClock aria-hidden /></span>}
              title="At a time"
              description="A schedule: what an agent does, which agent, and when; the hours it may run in, a check to look first, its time zone and skills."
              trailing={
                <Button variant="secondary" size="sm" leadingIcon={<Plus />} onClick={() => setAdding(true)} disabled={!online}>
                  Add schedule
                </Button>
              }
            />
            <ListRow
              align="center"
              leading={<span className="wat-icon"><Eye aria-hidden /></span>}
              title="When something turns up in an app"
              description="A watch: which app, what to look for, how often and how loud, and an agent to act on each new one. Ghost writes it and asks you first."
              trailing={
                <Button variant="secondary" size="sm" leadingIcon={<Plus />} onClick={() => setAddingWatch(true)} disabled={!online}>
                  Add watch
                </Button>
              }
            />
            <ListRow
              align="center"
              leading={<span className="wat-icon"><Webhook aria-hidden /></span>}
              title="When a service tells Ghost"
              description={
                <>
                  A webhook, declared in <span className="t-mono">hooks.json</span> in Ghost’s folder with recorded deliveries in <span className="t-mono">hooks/&lt;name&gt;/</span>; <span className="t-mono">docs/folder.md</span> shows the shape.{" "}
                  {watching && watching.listening.length > 0 ? (
                    <>
                      Ghost takes them on <span className="t-mono">{listWords(watching.listening)}</span>.
                    </>
                  ) : (
                    <>
                      The listener is off: set <span className="t-mono">GHOST_HOOKS_LISTEN</span> in Ghost’s settings to turn it on.
                    </>
                  )}
                </>
              }
              trailing={
                <Button variant="secondary" size="sm" leadingIcon={<FolderOpen />} onClick={() => void openFolder()}>
                  Open folder
                </Button>
              }
            />
          </Card>
          <p className="wat-files t-small t-3">
            The files: <span className="t-mono">{CONFIG_DIR}/watches.json</span> and <span className="t-mono">hooks.json</span>. They are yours. An agent may add a watch of its own with your yes, marked with who made it; Ghost never changes your own entries or the webhooks. Schedules are kept by Ghost, not in a file.
          </p>
        </Disclosure>
      </div>

      <ScheduleDialog
        open={adding || editing != null}
        onClose={() => (setAdding(false), setEditing(null))}
        agents={agents}
        defaultAgent={only}
        zone={schedules?.time_zone ?? ""}
        editing={editing}
        onSaved={(s) => {
          const added = editing == null;
          setAdding(false);
          setEditing(null);
          setSchedules((v) => (v ? { ...v, schedules: added ? [...v.schedules, s] : v.schedules.map((x) => (x.id === s.id ? s : x)) } : v));
          toast({ title: added ? "Added" : "Saved", description: `${nameOf(s.agent)} runs it ${untilWords(s.next_run) || "at its time"}.` });
        }}
      />
      <AddWatchDialog open={addingWatch} onClose={() => setAddingWatch(false)} agents={agents} />
      <RunsDialog title={history?.title ?? null} load={history?.load ?? null} onClose={() => setHistory(null)} />
      <Dialog
        open={problem != null}
        onClose={() => setProblem(null)}
        size="md"
        title="Why it can’t run"
        description={problem?.sentence}
        footer={
          <Button variant="secondary" onClick={() => setProblem(null)}>
            Close
          </Button>
        }
      >
        {problem && (
          <dl className="wat-details">
            {problem.file && (
              <>
                <dt>File</dt>
                <dd className="t-mono">{problem.file}</dd>
              </>
            )}
            <dt>Ghost says</dt>
            <dd className="t-mono t-small" data-user-content>
              {problem.detail}
            </dd>
          </dl>
        )}
      </Dialog>
      <DetailsDialog d={details} listening={watching?.listening ?? []} nameOf={nameOf} appTitle={appTitle} onClose={() => setDetails(null)} />
      {confirmDialog}
    </Page>
  );
}

/** The agent that made a watch, from `agent:<slug>`. */
const madeBySlug = (w: WatchInfo) => (w.made_by ?? "").replace(/^agent:/, "");

/** A watch's or a webhook's details: what the old Watching rows said, in full. */
function DetailsDialog({ d, listening, nameOf, appTitle, onClose }: { d: Doing | null; listening: string[]; nameOf: (s: string) => string; appTitle: (n: string) => string; onClose: () => void }) {
  return (
    <Dialog
      open={d != null && d.kind !== "schedule"}
      onClose={onClose}
      size="md"
      title={d?.kind === "watch" ? `The watch ${d.w.name}` : d?.kind === "hook" ? `The webhook ${d.h.name}` : ""}
      description={d?.kind === "watch" ? "It reads a connected app on its own and keeps only the fields it names. One failure is one item on Today, however often it is seen." : "A delivery is checked for its signature before Ghost reads it."}
      footer={
        <Button variant="secondary" onClick={onClose}>
          Close
        </Button>
      }
    >
      {d?.kind === "watch" && <WatchDetails w={d.w} nameOf={nameOf} appTitle={appTitle} />}
      {d?.kind === "hook" && <HookDetails h={d.h} listening={listening} />}
    </Dialog>
  );
}

function WatchDetails({ w, nameOf, appTitle }: { w: WatchInfo; nameOf: (s: string) => string; appTitle: (n: string) => string }) {
  const by = w.made_by ? madeBy(w.made_by) : null;
  return (
    <dl className="wat-details">
      <dt>Reads</dt>
      <dd>
        <span className="t-mono">{`${w.app}.${w.read}`}</span> in {appTitle(w.app)}, {everyWords(w.every_secs)}
      </dd>
      <dt>Decides with</dt>
      <dd>{stepWords(w.steps)}</dd>
      <dt>Then</dt>
      <dd>
        {RESPONSE[w.response]}
        {w.response === "tell" ? ` ${URGENCY[w.urgency]}` : ""}
        {w.acts_through ? `, and ${nameOf(w.acts_through)} looks into each new one` : ""}
      </dd>
      <dt>Set up by</dt>
      <dd>{!by || by.you ? "You, in watches.json" : "tool" in by ? toolTitle(by.tool) : nameOf(by.slug)}</dd>
      <dt>So far</dt>
      <dd className="t-num">
        {w.open} open · {w.seen} seen · {w.paused ? "not looking" : w.last_run ? `looked ${relativeTime(w.last_run)}` : "no look yet"}
        {w.last_ok ? ` · last worked ${relativeTime(w.last_ok)}` : ""}
      </dd>
      {w.error && (
        <>
          <dt>Can’t read</dt>
          <dd className="wat-error" data-user-content>
            {w.error}
          </dd>
        </>
      )}
    </dl>
  );
}

function HookDetails({ h, listening }: { h: HookInfo; listening: string[] }) {
  const taken = sumOf(h.accepted);
  const refused = sumOf(h.rejected);
  const accepted = Object.entries(h.accepted)
    .filter(([, n]) => n > 0)
    .map(([k, n]) => (k === "item" ? count(n, "item") : `${n} ${k}`))
    .join(", ");
  return (
    <dl className="wat-details">
      <dt>Takes</dt>
      <dd>
        <span className="t-mono">POST {h.path}</span>
        {listening.length > 0 ? (
          <>
            {" "}
            on <span className="t-mono">{listWords(listening)}</span>
          </>
        ) : (
          ", but the listener is off"
        )}
      </dd>
      <dt>Checked by</dt>
      <dd>{VERIFY[h.verify]}</dd>
      <dt>Counts</dt>
      <dd>
        {h.events.length
          ? h.events.map((ev, i) => (
              <Fragment key={ev}>
                {i > 0 && (i === h.events.length - 1 ? " and " : ", ")}
                <span className="t-mono">{ev}</span>
              </Fragment>
            ))
          : "No events declared."}
      </dd>
      <dt>State</dt>
      <dd>{h.on ? "On" : `Off: ${h.off_reason ?? "the listener is off"}`}</dd>
      <dt>So far</dt>
      <dd className="t-num">
        {taken} taken · {refused} refused · {h.last ? `last ${relativeTime(h.last)}` : "nothing yet"}
        {accepted ? ` · ${accepted}` : ""}
      </dd>
      {refused > 0 && (
        <>
          <dt>Refused</dt>
          <dd>{reasonWords(h.rejected)}</dd>
        </>
      )}
    </dl>
  );
}
