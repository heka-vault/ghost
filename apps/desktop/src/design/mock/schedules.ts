/**
 * The mock daemon's schedules (ADR 0047, `ghost_core::schedules`): the list, adding and
 * changing one, pause and resume, remove, run now and a schedule's runs. A run started here
 * finishes after a moment with a made-up answer and puts an item on the waiting list, as the
 * daemon's does. Refusals use the daemon's own sentences.
 */
import type { ScheduleInfo, ScheduleRequest, ScheduleRunInfo, SchedulesView } from "@ghost/client-ts";

import { HttpError } from "./errors";

export interface ScheduleHooks {
  /** The agents Ghost has, by slug: `retired` when it is. */
  agent(slug: string): "known" | "retired" | "unknown";
  /** A run's answer, for the waiting list. */
  deliver(agent: string, key: string, title: string): void;
  later(fn: () => void, ms: number): void;
  /** A run started or ended (ADR 0060): the daemon's `run` event. */
  ran?(run: ScheduleRunInfo, phase: "started" | "ended"): void;
}

const DURATION = /^(\d+)\s*(m|min|h|d|w)?$/i;
const PER: Record<string, number> = { m: 1, min: 1, h: 60, d: 1440, w: 10080 };

/** `30m`, `2h`, `1d`: minutes. */
function minutesOf(text: string): number | null {
  const m = DURATION.exec(text.trim());
  if (!m) return null;
  return Number(m[1]) * (PER[(m[2] ?? "m").toLowerCase()] ?? 1);
}

const inMinutes = (minutes: number) => new Date(Date.now() + minutes * 60_000).toISOString();
const now = () => new Date().toISOString();
const id = () => Math.random().toString(16).slice(2, 14).padEnd(12, "0");

export class MockSchedules {
  private view: SchedulesView;
  private runs: Record<string, ScheduleRunInfo[]>;

  constructor(
    f: { view: SchedulesView; runs: Record<string, ScheduleRunInfo[]> },
    private hooks: ScheduleHooks,
  ) {
    this.view = structuredClone(f.view);
    this.runs = structuredClone(f.runs);
  }

  /** Every run, newest first. */
  allRuns(): ScheduleRunInfo[] {
    return Object.values(this.runs)
      .flat()
      .sort((a, b) => b.scheduled_for.localeCompare(a.scheduled_for));
  }

  private setRun(runId: string, patch: Partial<ScheduleRunInfo>): ScheduleRunInfo | null {
    let hit: ScheduleRunInfo | null = null;
    for (const [k, list] of Object.entries(this.runs))
      this.runs[k] = list.map((x) => {
        if (x.id !== runId) return x;
        hit = { ...x, ...patch };
        return hit;
      });
    return hit;
  }

  private find(scheduleId: string): ScheduleInfo {
    const s = this.view.schedules.find((x) => x.id === scheduleId);
    if (!s) throw new HttpError(404, `there is no schedule ${scheduleId}`);
    return s;
  }

  private put(s: ScheduleInfo): ScheduleInfo {
    this.view.schedules = this.view.schedules.map((x) => (x.id === s.id ? s : x));
    return structuredClone(s);
  }

  /** When, from a request: the kind, its fields, the summary and the next run. */
  private when(req: ScheduleRequest, zone: string): Pick<ScheduleInfo, "kind" | "at" | "every" | "cron" | "summary" | "next_run"> {
    const given = [req.at, req.every, req.cron].filter((v) => typeof v === "string" && v.trim()).length;
    if (given === 0) throw new HttpError(422, "a schedule is one of at (a time), every (30m, 2h, 1d) or cron (0 9 * * 1-5)");
    if (given > 1) throw new HttpError(422, "a schedule is one of at, every or cron, not two of them");
    if (req.every) {
      const minutes = minutesOf(req.every);
      if (minutes == null) throw new HttpError(422, `${req.every} is not a length of time (30m, 2h, 1d)`);
      if (minutes < 1) throw new HttpError(422, "every is a minute at the least and a year at the most");
      return { kind: "every", at: null, every: req.every.trim(), cron: null, summary: `every ${req.every.trim()}`, next_run: inMinutes(minutes) };
    }
    if (req.cron) {
      const expr = req.cron.trim().split(/\s+/).join(" ");
      if (expr.split(" ").length !== 5) throw new HttpError(422, `${req.cron} is not a cron expression (five fields: minute hour day month weekday)`);
      return { kind: "cron", at: null, every: null, cron: expr, summary: `${expr} (${zone})`, next_run: inMinutes(60 * 9) };
    }
    const text = (req.at ?? "").trim();
    const delay = minutesOf(text);
    const at = delay != null ? new Date(Date.now() + delay * 60_000) : new Date(text);
    if (Number.isNaN(at.getTime())) throw new HttpError(422, `${text} is not a time (2026-11-12T09:00, a time with an offset, or a delay like 20m)`);
    if (at.getTime() <= Date.now()) throw new HttpError(422, "that time has passed, or never comes");
    const iso = at.toISOString();
    return { kind: "at", at: iso, every: null, cron: null, summary: `at ${iso.slice(0, 16).replace("T", " ")}`, next_run: iso };
  }

  private agentOf(slug: string): string {
    const state = this.hooks.agent(slug);
    if (state === "unknown") throw new HttpError(422, `there is no agent ${slug}`);
    if (state === "retired") throw new HttpError(422, `the agent ${slug} is retired; rehire it first`);
    return slug;
  }

  private add(req: ScheduleRequest): ScheduleInfo {
    const instruction = (req.instruction ?? "").trim();
    if (!instruction) throw new HttpError(422, "a schedule needs an instruction");
    const zone = req.time_zone?.trim() || this.view.time_zone;
    const when = this.when(req, zone);
    const s: ScheduleInfo = {
      id: id(),
      name: req.name?.trim() || instruction.slice(0, 48),
      agent: this.agentOf(req.agent?.trim() || "ghost"),
      instruction,
      ...when,
      time_zone: zone,
      enabled: true,
      created_by: "operator",
      created_at: now(),
      updated_at: now(),
      last_run: null,
      last_outcome: null,
      last_error: null,
      running: false,
      active_hours: req.active_hours ?? null,
      grace_minutes: req.grace_minutes ?? 120,
      max_runtime_minutes: req.max_runtime_minutes ?? 10,
      max_turns: req.max_turns ?? 30,
      delete_after_run: req.delete_after_run ?? when.kind === "at",
      skills: req.skills ?? [],
      urgency: req.urgency ?? "notify",
      notify_failure: req.notify_failure ?? true,
      check: req.check ?? null,
    };
    this.view.schedules = [...this.view.schedules, s];
    return structuredClone(s);
  }

  private edit(s: ScheduleInfo, req: ScheduleRequest): ScheduleInfo {
    const timed = req.at != null || req.every != null || req.cron != null;
    return this.put({
      ...s,
      name: req.name?.trim() || s.name,
      agent: req.agent ? this.agentOf(req.agent.trim()) : s.agent,
      instruction: req.instruction?.trim() || s.instruction,
      ...(timed ? this.when(req, req.time_zone?.trim() || s.time_zone) : {}),
      active_hours: req.clear_active_hours ? null : (req.active_hours ?? s.active_hours),
      urgency: req.urgency ?? s.urgency,
      notify_failure: req.notify_failure ?? s.notify_failure,
      check: req.clear_check ? null : (req.check ?? s.check),
      updated_at: now(),
    });
  }

  private run(s: ScheduleInfo): ScheduleRunInfo {
    const going = this.runs[s.id]?.find((r) => r.status === "queued" || r.status === "running");
    if (going) return structuredClone(going);
    const r: ScheduleRunInfo = {
      id: id(),
      schedule: s.id,
      source: "run_now",
      agent: s.agent,
      name: s.name,
      status: "queued",
      scheduled_for: now(),
      started_at: null,
      finished_at: null,
      answer: null,
      error: null,
      delivery: null,
      retries: 0,
      model_calls: 0,
      tool_calls: 0,
    };
    this.runs[s.id] = [r, ...(this.runs[s.id] ?? [])];
    this.put({ ...s, running: true });
    this.hooks.later(() => {
      const started = this.setRun(r.id, { status: "running", started_at: now() });
      if (started && started.status === "running") this.hooks.ran?.(structuredClone(started), "started");
    }, 300);
    this.hooks.later(() => {
      const going = this.allRuns().find((x) => x.id === r.id);
      if (!going || going.status !== "running") return;
      const answer = "Nothing unusual: everything this looks at is as it was this morning.";
      const ended = this.setRun(r.id, { status: "ok", finished_at: now(), answer, delivery: "delivered", model_calls: 2, tool_calls: 3 });
      if (ended) this.hooks.ran?.(structuredClone(ended), "ended");
      const current = this.view.schedules.find((x) => x.id === s.id);
      if (current) this.put({ ...current, running: false, last_run: now(), last_outcome: "ok", last_error: null });
      this.hooks.deliver(s.agent, `agent:${s.agent}:schedule:${s.id}`, `${s.name}: ${answer}`);
    }, 4500);
    return structuredClone(r);
  }

  handle(method: string, path: string, query: URLSearchParams, b: Record<string, unknown>): unknown {
    if (!path.startsWith("/schedules") && !path.startsWith("/runs")) return undefined;
    const req = b as ScheduleRequest;
    let r: RegExpExecArray | null;
    if (method === "GET" && path === "/schedules") return structuredClone(this.view);
    if (method === "POST" && path === "/schedules") return this.add(req);
    if ((r = /^\/schedules\/([^/]+)$/.exec(path))) {
      const s = this.find(r[1]);
      if (method === "GET") return structuredClone(s);
      if (method === "PATCH") return this.edit(s, req);
      if (method === "DELETE") {
        this.view.schedules = this.view.schedules.filter((x) => x.id !== s.id);
        delete this.runs[s.id];
        return { ok: true, detail: null };
      }
    }
    if (method === "POST" && (r = /^\/schedules\/([^/]+)\/(pause|resume|run)$/.exec(path))) {
      const s = this.find(r[1]);
      if (r[2] === "run") return this.run(s);
      if (r[2] === "pause") return this.put({ ...s, enabled: false, next_run: null, updated_at: now() });
      if (s.kind === "at" && s.at && Date.parse(s.at) <= Date.now()) throw new HttpError(422, "its time has passed; give it a new one to resume it");
      const next = s.kind === "every" ? inMinutes(minutesOf(s.every ?? "") ?? 60) : s.kind === "at" ? s.at : inMinutes(60 * 9);
      return this.put({ ...s, enabled: true, next_run: next ?? null, updated_at: now() });
    }
    if (method === "GET" && (r = /^\/schedules\/([^/]+)\/runs$/.exec(path))) {
      const s = this.find(r[1]);
      const limit = Number(query.get("limit") ?? 20);
      return { runs: structuredClone((this.runs[s.id] ?? []).slice(0, limit)) };
    }
    if (method === "GET" && path === "/runs") {
      // Since and until narrow them by when they were due (ADR 0060).
      const since = query.get("since"), until = query.get("until");
      const all = this.allRuns().filter((x) => (!since || x.scheduled_for >= since) && (!until || x.scheduled_for < until));
      return { runs: structuredClone(all.slice(0, Number(query.get("limit") ?? 20))) };
    }
    if (method === "POST" && (r = /^\/runs\/([^/]+)\/cancel$/.exec(path))) {
      const going = this.allRuns().find((x) => x.id === r![1] && (x.status === "running" || x.status === "queued"));
      if (!going) throw new HttpError(404, `there is no run ${r[1]} waiting or going`);
      const ended = this.setRun(going.id, { status: "cancelled", finished_at: now(), delivery: "none" });
      if (going.schedule) {
        const sched = this.view.schedules.find((x) => x.id === going.schedule);
        if (sched) this.put({ ...sched, running: false, last_outcome: "cancelled" });
      }
      if (ended && going.status === "running") this.hooks.ran?.(structuredClone(ended), "ended");
      return { ok: true, detail: null };
    }
    return undefined;
  }
}
