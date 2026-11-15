/**
 * Owner: watching. A schedule's form and its history (ADR 0047), for On its own: the form adds
 * or changes one (what to do, who, when, how loud, and under More: the hours it may run in, a
 * check to look first, its time zone and the skills it starts with), reached from Set it up by
 * hand and a row's Edit…; `RunsDialog` is what a row's runs did, newest first. The list itself
 * is `index.tsx`. Every write waits for Ghost to be reachable.
 */
import type { AppView, ConversationView, ScheduleInfo, ScheduleRequest, ScheduleRunInfo, SkillInfo } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { History } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Button, Checkbox, Dialog, Disclosure, EmptyState, Field, Input, Notice, SegmentedControl, Select, Skeleton, Textarea, agentName, errorText, useLive } from "../../ui";
import { RunCard, useAppTitles, useRunReach } from "./Runs";
import { WhenPicker } from "./WhenPicker";
import { PLAIN_DEFAULT, type Plain, plainOf, requestOf } from "./when";

export interface DialogProps {
  open: boolean;
  onClose: () => void;
  agents: ConversationView[];
  /** The zone a time is read in. */
  zone: string;
  /** Who does a new one, when the page is showing one agent. */
  defaultAgent?: string | null;
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
  /** When it runs, as the plain choice (or the cron form) that says it. */
  plain: Plain;
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

function draftOf(s: ScheduleInfo | null | undefined, agent?: string | null): Draft {
  return {
    instruction: s?.instruction ?? "",
    name: s?.name ?? "",
    agent: s?.agent ?? agent ?? "ghost",
    plain: s ? plainOf(s.kind, whenOf(s)) : PLAIN_DEFAULT,
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

export function ScheduleDialog({ open, onClose, agents, defaultAgent, zone, editing, onSaved }: DialogProps) {
  const { client } = useGhost();
  const { online } = useLive();
  const [d, setD] = useState<Draft>(() => draftOf(editing, defaultAgent));
  const [apps, setApps] = useState<AppView[]>([]);
  const [skills, setSkills] = useState<SkillInfo[]>([]);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const first = useRef<HTMLTextAreaElement>(null);
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => setD((x) => ({ ...x, [k]: v }));

  useEffect(() => {
    if (!open) return;
    setD(draftOf(editing, defaultAgent));
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
  }, [open, editing, client, defaultAgent]);

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
  const when = requestOf(d.plain);
  const whenProblem = "problem" in when ? when.problem : null;
  const ready = d.instruction.trim() !== "" && !whenProblem && !hoursProblem && !argsProblem && !checkProblem;

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
    const wasWhen = editing ? { kind: editing.kind, when: whenOf(editing).trim() } : null;
    if (!("problem" in when) && (!wasWhen || wasWhen.kind !== when.kind || wasWhen.when !== when.when)) req[when.kind] = when.when;
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
        <WhenPicker value={d.plain} onChange={(p) => (set("plain", p), setError(null))} problem={error} zone={d.zone.trim() || zone} />
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

/** What one row's runs did, newest first: a schedule's, or a watch's that acts. */
export function RunsDialog({ title, load, onClose }: { title: string | null; load: (() => Promise<ScheduleRunInfo[]>) | null; onClose: () => void }) {
  const [runs, setRuns] = useState<ScheduleRunInfo[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const reach = useRunReach(runs);
  const appTitle = useAppTitles();

  useEffect(() => {
    if (!load) return;
    let on = true;
    setRuns(null);
    setError(null);
    load()
      .then((r) => on && setRuns(r))
      .catch((e) => on && setError(errorText(e)));
    return () => {
      on = false;
    };
  }, [load]);

  return (
    <Dialog
      open={load != null}
      onClose={onClose}
      size="md"
      title={title ? `What ${title} did` : "What it did"}
      description="Its last runs, newest first: how each ended, how long it took, what it reached and what it said."
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
        <div className="wat-runs">
          {runs.map((r, i) => (
            <RunCard key={r.id} r={r} reach={reach.get(r.id)} appTitle={appTitle} open={i === 0} hideName />
          ))}
        </div>
      )}
    </Dialog>
  );
}
