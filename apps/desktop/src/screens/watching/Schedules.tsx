/**
 * Owner: watching. Schedules (ADR 0047), from GET /schedules: what each agent does on its
 * own and when, with its next run, how the last one ended, a switch to pause it, Run now,
 * Stop for a run that is going, what its last runs said, Edit, Remove, and a form to add
 * or change one (what to do, who, when, how loud, and under More: the hours it may run in,
 * a check to look first, its time zone and the skills it starts with). A schedule an agent
 * set up itself shows here too, with who made it. Every write waits for Ghost to be
 * reachable.
 */
import type { AppView, ConversationView, ScheduleInfo, ScheduleRequest, ScheduleRunInfo, SchedulesView, SkillInfo } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { CalendarClock, History, MoreHorizontal, Pencil, Play, Plus, Square, Trash2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  Badge,
  Button,
  Card,
  Checkbox,
  Dialog,
  Disclosure,
  EmptyState,
  Field,
  IconButton,
  Input,
  ListRow,
  Menu,
  Notice,
  Section,
  SegmentedControl,
  Select,
  Skeleton,
  Switch,
  Textarea,
  agentName,
  errorText,
  relativeTime,
  useConfirm,
  useLive,
  useToast,
} from "../../ui";
import { toolTitle } from "../settings/tools";
import { DELIVERY, KINDS, OUTCOME, type ScheduleKind, URGENCY, count, madeBy, untilWords, whenWords } from "./model";

/** How often the list is read again while the screen is open: next and last runs move. */
const REFRESH_MS = 15_000;

export function Schedules() {
  const { client, waitingVersion } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog: confirmDialog } = useConfirm();
  const [view, setView] = useState<SchedulesView | null>(null);
  const [agents, setAgents] = useState<ConversationView[]>([]);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [failed, setFailed] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);
  const [adding, setAdding] = useState(false);
  const [editing, setEditing] = useState<ScheduleInfo | null>(null);
  const [runsOf, setRunsOf] = useState<ScheduleInfo | null>(null);
  const [tick, setTick] = useState(0);

  useEffect(() => {
    const id = setInterval(() => setTick((n) => n + 1), REFRESH_MS);
    return () => clearInterval(id);
  }, []);

  useEffect(() => {
    let on = true;
    client
      .schedules()
      .then((v) => on && (setView(v), setLoadError(null)))
      .catch((e) => on && setLoadError(errorText(e)));
    return () => {
      on = false;
    };
    // A run's answer lands on Today: read the list again when that changes.
  }, [client, waitingVersion, tick]);

  useEffect(() => {
    let on = true;
    client
      .conversations(false)
      .then((list) => on && setAgents(list))
      .catch(() => {});
    return () => {
      on = false;
    };
  }, [client]);

  const nameOf = useCallback((slug: string) => agentName(slug, agents.find((a) => a.slug === slug)?.name), [agents]);
  const put = (s: ScheduleInfo) => setView((v) => (v ? { ...v, schedules: v.schedules.map((x) => (x.id === s.id ? s : x)) } : v));

  /** One write: the row is busy while it goes, and a refusal is said above the list. */
  const act = async (s: ScheduleInfo, what: string, fn: () => Promise<void>) => {
    if (busy) return;
    setBusy(s.id);
    setFailed(null);
    try {
      await fn();
    } catch (e) {
      setFailed(`Ghost couldn’t ${what} ${s.name}: ${errorText(e)}`);
    } finally {
      setBusy(null);
    }
  };

  const toggle = (s: ScheduleInfo, enabled: boolean) => act(s, enabled ? "resume" : "pause", async () => put(enabled ? await client.resumeSchedule(s.id) : await client.pauseSchedule(s.id)));

  const runNow = (s: ScheduleInfo) =>
    act(s, "run", async () => {
      await client.runSchedule(s.id);
      put({ ...s, running: true });
      toast({ title: "Started", description: `${nameOf(s.agent)} is on it. The answer goes to Today.` });
    });

  /** Stop the run that is going (or waiting): it records that it was stopped. */
  const stop = (s: ScheduleInfo) =>
    act(s, "stop", async () => {
      const { runs } = await client.scheduleRuns(s.id, 5);
      const going = runs.find((r) => r.status === "running" || r.status === "queued");
      if (!going) throw new Error("no run of it is going now");
      await client.cancelRun(going.id);
      put({ ...s, running: false });
      toast({ title: "Stopped", description: `${nameOf(s.agent)} stops where it is. What it already changed is in the audit.` });
    });

  const remove = async (s: ScheduleInfo) => {
    const ok = await confirm({
      title: `Remove ${s.name}?`,
      description: "It stops running, and what its runs said goes with it. What it already put on Today stays.",
      confirmLabel: `Remove ${s.name}`,
      tone: "danger",
    });
    if (!ok) return;
    await act(s, "remove", async () => {
      await client.removeSchedule(s.id);
      setView((v) => (v ? { ...v, schedules: v.schedules.filter((x) => x.id !== s.id) } : v));
    });
  };

  return (
    <Section
      id="wat-schedules"
      title="Schedules"
      description={<>What your agents do on their own, with nobody there. Each run uses the agent’s tools under your permissions, and its answer goes to Today.</>}
      actions={
        view != null && view.schedules.length > 0 ? (
          <Button variant="secondary" size="sm" leadingIcon={<Plus />} onClick={() => setAdding(true)} disabled={!online}>
            Add schedule
          </Button>
        ) : undefined
      }
    >
      {failed && (
        <Notice tone="danger" title="That didn’t happen" onDismiss={() => setFailed(null)} className="wat-notice">
          {failed}
        </Notice>
      )}
      {view != null && view.problems.length > 0 && (
        <Notice tone="warn" title={`${count(view.problems.length, "schedule")} can’t run`} className="wat-notice">
          <ul className="wat-problems">
            {view.problems.map((p) => (
              <li key={p} className="t-small" data-user-content>
                {p}
              </li>
            ))}
          </ul>
        </Notice>
      )}
      {view == null && loadError ? (
        <Notice tone="danger" title="Couldn’t load the schedules">
          {loadError}
        </Notice>
      ) : view == null ? (
        <Card list aria-busy="true" aria-label="Loading">
          {[0, 1].map((i) => (
            <div key={i} className="wat-skel-row">
              <Skeleton width={20} height={20} radius="var(--radius-sm)" />
              <span className="stack gap-2 grow">
                <Skeleton width={i ? 120 : 160} height={12} />
                <Skeleton width={i ? 240 : 300} height={10} />
              </span>
            </div>
          ))}
        </Card>
      ) : view.schedules.length === 0 ? (
        <Card>
          <EmptyState
            size="compact"
            icon={<CalendarClock />}
            title="Nothing is scheduled"
            description={<>Add one here, or tell an agent: “check the build every morning at nine and tell me if it’s red.” It sets the schedule up itself, and it shows here.</>}
            actions={
              <Button leadingIcon={<Plus />} onClick={() => setAdding(true)} disabled={!online}>
                Add schedule
              </Button>
            }
          />
        </Card>
      ) : (
        <Card list>
          {view.schedules.map((s) => (
            <ScheduleRow
              key={s.id}
              s={s}
              zone={view.time_zone}
              agent={nameOf(s.agent)}
              maker={nameOf}
              busy={busy === s.id}
              online={online}
              onToggle={(on) => void toggle(s, on)}
              onRun={() => void runNow(s)}
              onStop={() => void stop(s)}
              onEdit={() => setEditing(s)}
              onRuns={() => setRunsOf(s)}
              onRemove={() => void remove(s)}
            />
          ))}
        </Card>
      )}
      <ScheduleDialog
        open={adding || editing != null}
        onClose={() => {
          setAdding(false);
          setEditing(null);
        }}
        agents={agents}
        zone={view?.time_zone ?? ""}
        editing={editing}
        onSaved={(s) => {
          const added = editing == null;
          setAdding(false);
          setEditing(null);
          setView((v) => (v ? { ...v, schedules: added ? [...v.schedules, s] : v.schedules.map((x) => (x.id === s.id ? s : x)) } : v));
          toast({ title: added ? "Added" : "Saved", description: `${nameOf(s.agent)} runs it ${untilWords(s.next_run) || "at its time"}.` });
        }}
      />
      <RunsDialog schedule={runsOf} onClose={() => setRunsOf(null)} />
      {confirmDialog}
    </Section>
  );
}

interface RowProps {
  s: ScheduleInfo;
  zone: string;
  agent: string;
  maker: (slug: string) => string;
  busy: boolean;
  online: boolean;
  onToggle: (on: boolean) => void;
  onRun: () => void;
  onStop: () => void;
  onEdit: () => void;
  onRuns: () => void;
  onRemove: () => void;
}

function ScheduleRow({ s, zone, agent, maker, busy, online, onToggle, onRun, onStop, onEdit, onRuns, onRemove }: RowProps) {
  const when = whenWords(s, zone);
  const by = madeBy(s.created_by);
  const last = s.last_outcome ? OUTCOME[s.last_outcome] : null;
  const state = s.running ? (
    <Badge tone="info" dot size="sm">
      Running
    </Badge>
  ) : !s.enabled ? (
    <Badge tone="neutral" dot size="sm">
      Paused
    </Badge>
  ) : last ? (
    <Badge tone={last.tone} dot size="sm">
      {last.label}
    </Badge>
  ) : (
    <Badge tone="neutral" dot size="sm">
      Not run yet
    </Badge>
  );
  return (
    <ListRow
      align="start"
      leading={
        <span className="wat-icon">
          <CalendarClock aria-hidden />
        </span>
      }
      title={
        <span className="wat-title">
          <span data-user-content>{s.name}</span>
          {state}
        </span>
      }
      description={
        <span className="wat-desc">
          <span>
            {agent} · <span title={when.detail}>{when.text}</span>
            {when.mono && (
              <>
                {" "}
                <span className="t-mono">{when.mono}</span>
              </>
            )}
            {s.active_hours ? `, between ${s.active_hours.start} and ${s.active_hours.end}` : ""}.
          </span>
          <span className="t-3">
            Tells you {URGENCY[s.urgency]}
            {s.check && (
              <>
                , and only when <span className="t-mono">{`${s.check.app}.${s.check.read}`}</span> has changed
              </>
            )}
            {by.you ? "" : ` · set up by ${"tool" in by ? toolTitle(by.tool) : maker(by.slug)}`}
          </span>
          {s.last_error && last && last.tone !== "ok" && (
            <span className="wat-error" data-user-content>
              {s.last_error}
            </span>
          )}
        </span>
      }
      trailing={
        <span className="wat-sched-end">
          <span className="wat-end">
            <span className="wat-end__counts t-num">{s.enabled ? (s.next_run ? `Next ${untilWords(s.next_run)}` : "No next run") : "Not running"}</span>
            <span className="wat-end__when">{s.last_run ? `ran ${relativeTime(s.last_run)}` : "no run yet"}</span>
          </span>
          <Switch checked={s.enabled} onChange={onToggle} disabled={busy || !online} size="sm" aria-label={`Run ${s.name} on its schedule`} />
          <Menu
            align="end"
            trigger={<IconButton label={`More for ${s.name}`} icon={<MoreHorizontal />} variant="ghost" size="sm" />}
            items={[
              s.running
                ? { label: "Stop this run", icon: <Square />, onSelect: onStop, disabled: busy || !online }
                : { label: "Run now", icon: <Play />, onSelect: onRun, disabled: busy || !online },
              { label: "What it said…", icon: <History />, onSelect: onRuns },
              { label: "Edit…", icon: <Pencil />, onSelect: onEdit, disabled: busy || !online },
              "separator",
              { label: "Remove…", icon: <Trash2 />, danger: true, onSelect: onRemove, disabled: busy || !online },
            ]}
          />
        </span>
      }
    />
  );
}

interface DialogProps {
  open: boolean;
  onClose: () => void;
  agents: ConversationView[];
  /** The zone a time is read in. */
  zone: string;
  /** The schedule being changed; none to add one. */
  editing?: ScheduleInfo | null;
  onSaved: (s: ScheduleInfo) => void;
}

type Urgency = ScheduleInfo["urgency"];

/** What the form keeps: every field of a schedule the operator may set. */
interface Draft {
  instruction: string;
  name: string;
  agent: string;
  kind: ScheduleKind;
  when: string;
  start: string;
  end: string;
  urgency: Urgency;
  zone: string;
  checkApp: string;
  checkRead: string;
  checkArgs: string;
  skills: string[];
}

const URGENCIES: { value: Urgency; label: string }[] = [
  { value: "digest", label: "In the summary" },
  { value: "notify", label: "Notification" },
  { value: "call", label: "Call" },
];

/** When a schedule runs, as the form writes it. */
function whenOf(s: ScheduleInfo): string {
  if (s.kind === "every") return s.every ?? "";
  if (s.kind === "cron") return s.cron ?? "";
  // A one-off's time, in this computer's local time, as the field takes it.
  if (!s.at) return "";
  const at = new Date(s.at);
  const pad = (n: number) => String(n).padStart(2, "0");
  return `${at.getFullYear()}-${pad(at.getMonth() + 1)}-${pad(at.getDate())}T${pad(at.getHours())}:${pad(at.getMinutes())}`;
}

function draftOf(s: ScheduleInfo | null | undefined): Draft {
  return {
    instruction: s?.instruction ?? "",
    name: s?.name ?? "",
    agent: s?.agent ?? "ghost",
    kind: s?.kind ?? "every",
    when: s ? whenOf(s) : "",
    start: s?.active_hours?.start ?? "",
    end: s?.active_hours?.end ?? "",
    urgency: s?.urgency ?? "notify",
    zone: s?.time_zone ?? "",
    checkApp: s?.check?.app ?? "",
    checkRead: s?.check?.read ?? "",
    checkArgs: s?.check && Object.keys(s.check.args ?? {}).length ? JSON.stringify(s.check.args, null, 2) : "",
    skills: s?.skills ?? [],
  };
}

const HOUR = /^([01]\d|2[0-3]):[0-5]\d$/;

function ScheduleDialog({ open, onClose, agents, zone, editing, onSaved }: DialogProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const [d, setD] = useState<Draft>(() => draftOf(editing));
  const [apps, setApps] = useState<AppView[]>([]);
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const first = useRef<HTMLTextAreaElement>(null);
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => setD((x) => ({ ...x, [k]: v }));

  useEffect(() => {
    if (!open) return;
    setD(draftOf(editing));
    setError(null);
    let on = true;
    // What a check may read: a connected app's read-only tools. And the skills to start with.
    client
      .apps()
      .then((a) => on && setApps(a.filter((x) => x.state === "connected" && x.actions.some((t) => t.reads))))
      .catch(() => {});
    client
      .skills()
      .then((k) => on && setSkills(k.filter((x) => !x.problem)))
      .catch(() => {});
    // showModal() focuses the close button; the instruction is where the operator starts.
    const id = requestAnimationFrame(() => first.current?.focus());
    return () => {
      on = false;
      cancelAnimationFrame(id);
    };
  }, [open, editing, client]);

  const hoursProblem = (d.start || d.end) && !(HOUR.test(d.start) && HOUR.test(d.end)) ? "Both as 24-hour times, like 08:00 and 18:00, or both empty for any hour." : null;
  let argsProblem: string | null = null;
  let args: Record<string, unknown> = {};
  if (d.checkArgs.trim()) {
    try {
      const parsed: unknown = JSON.parse(d.checkArgs);
      if (parsed && typeof parsed === "object" && !Array.isArray(parsed)) args = parsed as Record<string, unknown>;
      else argsProblem = "Its arguments are an object, like {“branch”: “main”}.";
    } catch {
      argsProblem = "That isn’t JSON. Its arguments are an object, like {“branch”: “main”}.";
    }
  }
  const checkProblem = d.checkApp && !d.checkRead ? "Pick which of its reads to look at." : null;
  const ready = d.instruction.trim() !== "" && d.when.trim() !== "" && !hoursProblem && !argsProblem && !checkProblem;

  const body = (): ScheduleRequest => {
    const was = editing ? draftOf(editing) : null;
    const req: ScheduleRequest = {
      instruction: d.instruction.trim(),
      name: d.name.trim() || null,
      agent: d.agent,
      urgency: d.urgency,
      skills: d.skills,
    };
    // A time is sent when it is new or changed: a one-off's time already past is not resent.
    if (!was || was.kind !== d.kind || was.when.trim() !== d.when.trim()) req[d.kind] = d.when.trim();
    if (d.zone.trim() && (!was || was.zone !== d.zone.trim())) req.time_zone = d.zone.trim();
    if (d.start && d.end) req.active_hours = { start: d.start, end: d.end };
    else if (was && was.start) req.clear_active_hours = true;
    if (d.checkApp && d.checkRead) req.check = { app: d.checkApp, read: d.checkRead, args };
    else if (was && was.checkApp) req.clear_check = true;
    return req;
  };

  const submit = async () => {
    if (!ready || busy) return;
    setBusy(true);
    setError(null);
    try {
      onSaved(editing ? await client.editSchedule(editing.id, body()) : await client.addSchedule(body()));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const options = [{ value: "ghost", label: "Ghost" }, ...agents.filter((a) => a.slug !== "ghost").map((a) => ({ value: a.slug, label: agentName(a.slug, a.name) }))];
  const reads = apps.find((a) => a.name === d.checkApp)?.actions.filter((t) => t.reads) ?? [];

  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title={editing ? `Edit ${editing.name}` : "Add schedule"}
      description="An agent carries this out on its own at the times you give, and tells you what it found."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="wat-schedule-form" loading={busy} disabled={!ready || !online}>
            {editing ? "Save" : "Add schedule"}
          </Button>
        </>
      }
    >
      <form
        id="wat-schedule-form"
        className="wat-form"
        onSubmit={(e) => {
          e.preventDefault();
          void submit();
        }}
      >
        <Field label="What to do" help="All a run is given, so write it to stand alone. Say when there is nothing to tell, and the run stays quiet.">
          <Textarea
            ref={first}
            rows={4}
            value={d.instruction}
            maxLength={4000}
            placeholder="Check the build on main. If it is red, say which job failed. If it is green, there is nothing to tell."
            onChange={(e) => set("instruction", e.target.value)}
          />
        </Field>
        <Field label="Who does it">
          <Select value={d.agent} onChange={(e) => set("agent", e.target.value)} options={options} />
        </Field>
        <Field label="When" error={error} help={`${KINDS[d.kind].help}${zone ? ` Times are in ${d.zone.trim() || zone}.` : ""}`}>
          <div className="wat-when">
            <SegmentedControl
              aria-label="How often"
              value={d.kind}
              onChange={(k) => {
                setD((x) => ({ ...x, kind: k, when: editing && editing.kind === k ? whenOf(editing) : "" }));
                setError(null);
              }}
              options={(Object.keys(KINDS) as ScheduleKind[]).map((k) => ({ value: k, label: KINDS[k].label }))}
            />
            <Input mono value={d.when} placeholder={KINDS[d.kind].placeholder} aria-label={KINDS[d.kind].label} onChange={(e) => set("when", e.target.value)} />
          </div>
        </Field>
        <Field label="Name" optional help="A few words to call it by. Left empty, it is the start of what to do.">
          <Input value={d.name} maxLength={80} placeholder="morning build check" onChange={(e) => set("name", e.target.value)} />
        </Field>
        <Field label="How it tells you" help="How loud its answer may get. Your quiet hours still hold.">
          <SegmentedControl aria-label="How it tells you" value={d.urgency} onChange={(u) => set("urgency", u)} options={URGENCIES} />
        </Field>
        <Disclosure key={editing?.id ?? "new"} summary="Hours, a check, the time zone and skills" defaultOpen={Boolean(editing && (editing.active_hours || editing.check || editing.skills.length))}>
          <div className="wat-form wat-more">
            <Field label="Only between" optional error={hoursProblem} help="Outside these hours a run waits for the next time inside them.">
              <div className="wat-pair">
                <Input mono value={d.start} placeholder="08:00" aria-label="From" onChange={(e) => set("start", e.target.value)} />
                <span className="t-small t-2">and</span>
                <Input mono value={d.end} placeholder="18:00" aria-label="Until" onChange={(e) => set("end", e.target.value)} />
              </div>
            </Field>
            <Field label="Look first" optional error={checkProblem ?? argsProblem} help="One of an app’s reads, called with no model. The agent runs only when what it returns has changed.">
              <div className="stack gap-2">
                <div className="wat-pair">
                  <Select
                    aria-label="App"
                    value={d.checkApp}
                    onChange={(e) => setD((x) => ({ ...x, checkApp: e.target.value, checkRead: "" }))}
                    options={[{ value: "", label: "No check" }, ...apps.map((a) => ({ value: a.name, label: a.title }))]}
                  />
                  <Select
                    aria-label="Its read"
                    value={d.checkRead}
                    disabled={!d.checkApp}
                    onChange={(e) => set("checkRead", e.target.value)}
                    options={[{ value: "", label: d.checkApp ? "Pick a read" : "Pick an app first" }, ...reads.map((t) => ({ value: t.name, label: t.name }))]}
                  />
                </div>
                {d.checkApp && <Textarea mono rows={2} value={d.checkArgs} aria-label="Its arguments, as JSON" placeholder={`{"branch": "main"}`} onChange={(e) => set("checkArgs", e.target.value)} />}
              </div>
            </Field>
            <Field label="Time zone" optional help={`An IANA name, like Europe/London. Left empty, ${zone || "this computer’s"}.`}>
              <Input mono value={d.zone} placeholder={zone || "America/Detroit"} onChange={(e) => set("zone", e.target.value)} />
            </Field>
            <fieldset className="wat-skills">
              <legend className="t-body">Skills to start with</legend>
              {skills.length === 0 ? (
                <p className="t-small t-3">No skills yet. They are on Settings › Skills.</p>
              ) : (
                skills.map((k) => (
                  <Checkbox
                    key={k.name}
                    label={k.name}
                    description={k.description}
                    checked={d.skills.includes(k.name)}
                    onChange={(on) => set("skills", on ? [...d.skills, k.name] : d.skills.filter((x) => x !== k.name))}
                  />
                ))
              )}
            </fieldset>
          </div>
        </Disclosure>
      </form>
    </Dialog>
  );
}

function RunsDialog({ schedule, onClose }: { schedule: ScheduleInfo | null; onClose: () => void }) {
  const { client } = useGhost();
  const [runs, setRuns] = useState<ScheduleRunInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const id = schedule?.id ?? null;

  useEffect(() => {
    if (!id) return;
    let on = true;
    setRuns(null);
    setError(null);
    client
      .scheduleRuns(id, 10)
      .then((r) => on && setRuns(r.runs))
      .catch((e) => on && setError(errorText(e)));
    return () => {
      on = false;
    };
  }, [client, id]);

  return (
    <Dialog
      open={schedule != null}
      onClose={onClose}
      size="md"
      title={schedule ? `What ${schedule.name} said` : "What it said"}
      description="Its last runs, newest first."
      footer={
        <Button variant="secondary" onClick={onClose}>
          Close
        </Button>
      }
    >
      {error ? (
        <Notice tone="danger" title="Couldn’t load its runs">
          {error}
        </Notice>
      ) : runs == null ? (
        <div className="stack gap-3" aria-busy="true" aria-label="Loading">
          <Skeleton width={220} height={12} />
          <Skeleton width={320} height={10} />
        </div>
      ) : runs.length === 0 ? (
        <EmptyState size="compact" icon={<History />} title="It hasn’t run yet" description="Its runs show here once it has, with what each one answered." />
      ) : (
        <ul className="wat-runs">
          {runs.map((r) => {
            const quiet = r.delivery === "silent" || r.delivery === "empty";
            return (
              <li key={r.id} className="wat-run">
                <span className="wat-run__head">
                  <Badge tone={OUTCOME[r.status].tone} dot size="sm">
                    {OUTCOME[r.status].label}
                  </Badge>
                  <span className="t-small t-3">
                    {relativeTime(r.finished_at ?? r.scheduled_for)}
                    {r.delivery && DELIVERY[r.delivery] ? ` · ${DELIVERY[r.delivery].toLowerCase()}` : ""}
                    {r.status === "ok" ? ` · ${count(r.tool_calls, "tool call")}` : ""}
                  </span>
                </span>
                {r.error ? (
                  <span className="wat-error" data-user-content>
                    {r.error}
                  </span>
                ) : r.answer && !quiet ? (
                  <span className="wat-run__answer" data-user-content>
                    {r.answer}
                  </span>
                ) : null}
              </li>
            );
          })}
        </ul>
      )}
    </Dialog>
  );
}
