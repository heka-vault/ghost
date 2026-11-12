/**
 * Owner: watching. Schedules (ADR 0047), from GET /schedules: what each agent does on its
 * own and when, with its next run, how the last one ended, a switch to pause it, Run now,
 * what its last runs said, Remove, and a short form to add one. A schedule an agent set up
 * itself shows here too, with who made it. Every write waits for Ghost to be reachable.
 */
import type { ConversationView, ScheduleInfo, ScheduleRunInfo, SchedulesView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { CalendarClock, History, MoreHorizontal, Play, Plus, Trash2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import {
  Badge,
  Button,
  Card,
  Dialog,
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
    // A run's answer lands on What needs you: read the list again when that changes.
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
      toast({ title: "Started", description: `${nameOf(s.agent)} is on it. The answer goes to What needs you.` });
    });

  const remove = async (s: ScheduleInfo) => {
    const ok = await confirm({
      title: `Remove ${s.name}?`,
      description: "It stops running, and what its runs said goes with it. What it already put on What needs you stays.",
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
      description={<>What your agents do on their own, with nobody there. Each run uses the agent’s tools under your permissions, and its answer goes to What needs you.</>}
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
              onRuns={() => setRunsOf(s)}
              onRemove={() => void remove(s)}
            />
          ))}
        </Card>
      )}
      <AddScheduleDialog
        open={adding}
        onClose={() => setAdding(false)}
        agents={agents}
        zone={view?.time_zone ?? ""}
        onAdded={(s) => {
          setAdding(false);
          setView((v) => (v ? { ...v, schedules: [...v.schedules, s] } : v));
          toast({ title: "Added", description: `${nameOf(s.agent)} runs it ${untilWords(s.next_run) || "at its time"}.` });
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
  onRuns: () => void;
  onRemove: () => void;
}

function ScheduleRow({ s, zone, agent, maker, busy, online, onToggle, onRun, onRuns, onRemove }: RowProps) {
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
            {agent} · {when.text}
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
              { label: "Run now", icon: <Play />, onSelect: onRun, disabled: busy || !online || s.running, reason: s.running ? "It is running" : undefined },
              { label: "What it said…", icon: <History />, onSelect: onRuns },
              "separator",
              { label: "Remove…", icon: <Trash2 />, danger: true, onSelect: onRemove, disabled: busy || !online },
            ]}
          />
        </span>
      }
    />
  );
}

interface AddProps {
  open: boolean;
  onClose: () => void;
  agents: ConversationView[];
  /** The zone a time is read in. */
  zone: string;
  onAdded: (s: ScheduleInfo) => void;
}

function AddScheduleDialog({ open, onClose, agents, zone, onAdded }: AddProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const [instruction, setInstruction] = useState("");
  const [name, setName] = useState("");
  const [agent, setAgent] = useState("ghost");
  const [kind, setKind] = useState<ScheduleKind>("every");
  const [when, setWhen] = useState("");
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const first = useRef<HTMLTextAreaElement>(null);

  useEffect(() => {
    if (!open) return;
    setInstruction("");
    setName("");
    setAgent("ghost");
    setKind("every");
    setWhen("");
    setError(null);
    // showModal() focuses the close button; the instruction is where the operator starts.
    const id = requestAnimationFrame(() => first.current?.focus());
    return () => cancelAnimationFrame(id);
  }, [open]);

  const ready = instruction.trim() !== "" && when.trim() !== "";
  const submit = async () => {
    if (!ready || busy) return;
    setBusy(true);
    setError(null);
    try {
      onAdded(await client.addSchedule({ instruction: instruction.trim(), name: name.trim() || null, agent, [kind]: when.trim() }));
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  const options = [{ value: "ghost", label: "Ghost" }, ...agents.filter((a) => a.slug !== "ghost").map((a) => ({ value: a.slug, label: agentName(a.slug, a.name) }))];

  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title="Add schedule"
      description="An agent carries this out on its own at the times you give, and tells you what it found."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="wat-add-schedule" loading={busy} disabled={!ready || !online}>
            Add schedule
          </Button>
        </>
      }
    >
      <form
        id="wat-add-schedule"
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
            value={instruction}
            maxLength={4000}
            placeholder="Check the build on main. If it is red, say which job failed. If it is green, there is nothing to tell."
            onChange={(e) => setInstruction(e.target.value)}
          />
        </Field>
        <Field label="Who does it">
          <Select value={agent} onChange={(e) => setAgent(e.target.value)} options={options} />
        </Field>
        <Field label="When" error={error} help={`${KINDS[kind].help}${zone ? ` Times are in ${zone}.` : ""}`}>
          <div className="wat-when">
            <SegmentedControl
              aria-label="How often"
              value={kind}
              onChange={(k) => {
                setKind(k);
                setWhen("");
                setError(null);
              }}
              options={(Object.keys(KINDS) as ScheduleKind[]).map((k) => ({ value: k, label: KINDS[k].label }))}
            />
            <Input mono value={when} placeholder={KINDS[kind].placeholder} aria-label={KINDS[kind].label} onChange={(e) => setWhen(e.target.value)} />
          </div>
        </Field>
        <Field label="Name" optional help="A few words to call it by. Left empty, it is the start of what to do.">
          <Input value={name} maxLength={80} placeholder="morning build check" onChange={(e) => setName(e.target.value)} />
        </Field>
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
