/**
 * Owner: watching. The list on On its own (the flow review, 2026-11-14): one list in two groups,
 * “At a time” and “When something happens”, every row with the same parts: when or on what (its
 * title) with its chip (Running, Paused, Failed, Can’t read, Can’t run, Off), what it does, its
 * state in a few words, a switch, and a ⋯ menu. What can't run is a chip and a sentence in its
 * own row, never a box above the list; a problem about something that isn't on the list is a row
 * of its own in its group. Files, intervals, signatures, counts of deliveries and passwords'
 * names are behind ⋯ (Details…). A row the window can't pause (the operator's own watch, a
 * webhook) shows its switch off-limits and says why in its menu, the same way every time.
 *
 * Presentational, with no data of its own, so test/plain-words.test.ts renders it from the busy
 * mock and reads every word it shows.
 */
import { History, Info, MoreHorizontal, Pause, Pencil, Play, Square, Trash2, Wrench } from "lucide-react";

import { Link, navigate } from "../../app/router";
import { Badge, Card, IconButton, LineGlyph, ListRow, Menu, Section, Switch, agentName } from "../../ui";
import { runErrorWords } from "./model";
import type { Problem } from "./problems";
import { type Doing, GROUP_TITLE, type Group, type ProblemRow, type Sentence, sentenceText } from "./sentence";

export interface ListHandlers {
  toggle: (d: Doing, on: boolean) => void;
  run: (d: Doing) => void;
  stop: (d: Doing) => void;
  edit: (d: Doing) => void;
  history: (d: Doing) => void;
  details: (d: Doing) => void;
  remove: (d: Doing) => void;
  /** A problem's own Details: the daemon's line. */
  problemDetails: (p: Problem) => void;
}

export interface ListProps {
  rows: { d: Doing; s: Sentence; problem: Problem | null }[];
  problems: ProblemRow[];
  busyKey: string | null;
  online: boolean;
  on: ListHandlers;
}

/** The list's two groups, each only when it has a row. */
export function OnItsOwnList({ rows, problems, busyKey, online, on }: ListProps) {
  return (
    <>
      {(["time", "event"] as Group[]).map((g) => {
        const mine = rows.filter((r) => r.s.group === g);
        const theirs = problems.filter((p) => p.group === g);
        if (!mine.length && !theirs.length) return null;
        return (
          <Section key={g} id={`wat-${g}`} title={GROUP_TITLE[g]} level={3} className="wat-group">
            <Card list>
              {mine.map((r) => (
                <DoingRow key={r.d.key} d={r.d} s={r.s} problem={r.problem} busy={busyKey === r.d.key} online={online} on={on} />
              ))}
              {theirs.map((p) => (
                <ProblemOnlyRow key={p.key} p={p.problem} on={on} />
              ))}
            </Card>
          </Section>
        );
      })}
    </>
  );
}

/** The chip a row wears: its problem's first, then its own state's. */
function Chip({ s, problem }: { s: Sentence; problem: Problem | null }) {
  if (problem)
    return (
      <Badge tone="danger" dot size="sm">
        {problem.chip}
      </Badge>
    );
  if (!s.badge) return null;
  return (
    <Badge tone={s.badge.tone} dot size="sm">
      {s.badge.label}
    </Badge>
  );
}

function DoingRow({ d, s, problem, busy, online, on }: { d: Doing; s: Sentence; problem: Problem | null; busy: boolean; online: boolean; on: ListHandlers }) {
  const off = busy || !online;
  const name = d.kind === "schedule" ? d.s.name : d.kind === "watch" ? d.w.name : d.h.name;
  const err = d.kind === "schedule" && d.s.last_error && s.badge ? d.s.last_error : null;
  const items = [
    ...(d.kind === "schedule"
      ? [
          d.s.running ? { label: "Stop this run", icon: <Square />, onSelect: () => on.stop(d), disabled: off } : { label: "Run now", icon: <Play />, onSelect: () => on.run(d), disabled: off },
          { label: "What it did…", icon: <History />, onSelect: () => on.history(d) },
          { label: "Edit…", icon: <Pencil />, onSelect: () => on.edit(d), disabled: off },
        ]
      : []),
    ...(d.kind === "watch" && d.w.acts_through ? [{ label: "What it did…", icon: <History />, onSelect: () => on.history(d) }] : []),
    ...(s.pause.why ? [{ label: s.pause.on ? "Pause" : "Turn on", icon: <Pause />, onSelect: () => undefined, disabled: true, reason: s.pause.why }] : []),
    ...(problem?.fix ? [{ label: problem.fix.label, icon: <Wrench />, onSelect: () => navigate(problem.fix!.path) }] : []),
    // A schedule's details are its Edit… and What it did…; a watch's and a webhook's are here.
    ...(d.kind !== "schedule" || problem ? [{ label: "Details…", icon: <Info />, onSelect: () => (problem ? on.problemDetails(problem) : on.details(d)) }] : []),
    ...(d.kind === "schedule" || (d.kind === "watch" && d.w.made_by != null) ? ["separator" as const, { label: "Remove…", icon: <Trash2 />, danger: true, onSelect: () => on.remove(d), disabled: off }] : []),
  ];
  return (
    <ListRow
      align="start"
      leading={<LineGlyph slug={s.agent} name={agentName(s.agent)} size="lg" />}
      title={
        <span className="wat-title">
          <span>{s.trigger}</span>
          <Chip s={s} problem={problem} />
        </span>
      }
      description={
        <span className="wat-desc">
          <span className="wat-what">
            {s.quote ? (
              <>
                {s.what}: <span data-user-content>{s.quote}</span>
              </>
            ) : (
              s.what
            )}
          </span>
          {problem ? <Reason p={problem} /> : <span className="t-3 t-num">{s.state}</span>}
          {err && !problem && (
            <span className="wat-error" title={err}>
              Last run didn’t finish: {runErrorWords(err)}.
            </span>
          )}
        </span>
      }
      trailing={
        <span className="wat-sched-end">
          <Switch
            checked={s.pause.on}
            onChange={(v: boolean) => on.toggle(d, v)}
            disabled={off || s.pause.why != null}
            size="sm"
            aria-label={s.pause.why ? `${sentenceText(s)}: ${s.pause.why}` : `Keep “${sentenceText(s)}” going`}
          />
          <Menu align="end" trigger={<IconButton label={`More for ${name}`} icon={<MoreHorizontal />} variant="ghost" size="sm" />} items={items} />
        </span>
      }
    />
  );
}

/** Why it can't run, with its fix where there is one place to fix it. */
function Reason({ p }: { p: Problem }) {
  return (
    <span className="wat-error">
      {p.reason}
      {p.fix && (
        <>
          {" "}
          <Link to={p.fix.path} className="wat-problem__fix">
            {p.fix.label}
          </Link>
        </>
      )}
    </span>
  );
}

/** Something the daemon can't read, so not on the list: a row of its own with the same parts. */
function ProblemOnlyRow({ p, on }: { p: Problem; on: ListHandlers }) {
  const title = p.subject ? p.subject.name.charAt(0).toUpperCase() + p.subject.name.slice(1).replace(/[-_]+/g, " ") : "Something in your setup";
  return (
    <ListRow
      align="start"
      leading={<LineGlyph slug="ghost" name="Ghost" size="lg" />}
      title={
        <span className="wat-title">
          <span>{title}</span>
          <Badge tone="danger" dot size="sm">
            {p.chip}
          </Badge>
        </span>
      }
      description={
        <span className="wat-desc">
          <Reason p={p} />
        </span>
      }
      trailing={
        <span className="wat-sched-end">
          <Switch checked={false} onChange={() => undefined} disabled size="sm" aria-label={`${title}: ${p.reason} It can’t be turned on until that is fixed.`} />
          <Menu
            align="end"
            trigger={<IconButton label={`More for ${title}`} icon={<MoreHorizontal />} variant="ghost" size="sm" />}
            items={[...(p.fix ? [{ label: p.fix.label, icon: <Wrench />, onSelect: () => navigate(p.fix!.path) }] : []), { label: "Details…", icon: <Info />, onSelect: () => on.problemDetails(p) }]}
          />
        </span>
      }
    />
  );
}
