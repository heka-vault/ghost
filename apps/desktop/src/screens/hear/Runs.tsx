/**
 * Owner: hear. Runs: score a session (POST /hear/runs) on ghost.mic or the raw array under it,
 * every run with its word error rates and a note of what it was for, and a run's report,
 * against a baseline when the operator picks one. A run holds the mic, so the screen opens
 * the recorder for it and keeps it open while the run is scored.
 */
import type { HearRun, HearRunInfo, HearSessionSummary, HearView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ChevronLeft, CloudOff, Gauge, MoreHorizontal, Pencil, Trash2 } from "lucide-react";
import { type KeyboardEvent, useEffect, useRef, useState } from "react";

import { Link, navigate } from "../../app/router";
import { Button, Card, EmptyState, Field, IconButton, Input, Menu, Notice, Progress, SegmentedControl, Select, Skeleton, SkeletonText, Table, errorText, useConfirm, useLive, useToast } from "../../ui";
import { pct, refusal } from "./model";
import { ReportView } from "./Report";

type Channel = "aec" | "raw";
const CHANNEL_WORDS: Record<string, string> = { aec: "ghost.mic", raw: "Raw array" };

interface Props {
  view: HearView | null;
  runId: string | null;
  /** A session the operator asked to score from Record or Sessions. */
  session: string | null;
  onScore: (session: string, channel: Channel) => Promise<HearRunInfo>;
}

export function RunsTab({ view, runId, session, onScore }: Props) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [runs, setRuns] = useState<HearRunInfo[] | null>(null);
  const [sessions, setSessions] = useState<HearSessionSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const scoring = view?.scoring ?? null;

  // Re-read the runs when a run finishes scoring.
  const [finished, setFinished] = useState(0);
  const was = useRef(!!scoring);
  useEffect(() => {
    if (was.current && !scoring) setFinished((n) => n + 1);
    was.current = !!scoring;
  }, [scoring]);

  useEffect(() => {
    let on = true;
    client
      .hearRuns()
      .then((r) => on && (setRuns(r), setError(null)))
      .catch((e) => on && setError(errorText(e)));
    client
      .hearSessions()
      .then((s) => on && setSessions(s))
      .catch(() => {});
    return () => {
      on = false;
    };
  }, [client, attempt, finished]);

  if (runId) return <RunDetail id={runId} runs={runs} />;

  const remove = async (r: HearRunInfo) => {
    const ok = await confirm({ title: `Delete run ${r.id}?`, description: "Its numbers and report go. The session it scored stays.", confirmLabel: `Delete ${r.id}`, tone: "danger" });
    if (!ok) return;
    try {
      await client.deleteHearRun(r.id);
      setRuns((l) => l?.filter((x) => x.id !== r.id) ?? null);
      toast({ title: "Deleted", description: `Run ${r.id} is gone.` });
    } catch (e) {
      toast({ title: "Couldn’t delete the run", description: refusal(e), tone: "danger" });
    }
  };
  const saveNote = async (r: HearRunInfo, note: string) => {
    try {
      const next = await client.noteHearRun(r.id, note);
      setRuns((l) => l?.map((x) => (x.id === r.id ? next : x)) ?? null);
      return true;
    } catch (e) {
      toast({ title: "Couldn’t save the note", description: refusal(e), tone: "danger" });
      return false;
    }
  };

  return (
    <div className="stack gap-8">
      <ScoreCard sessions={sessions} initial={session} scoring={scoring} problem={view?.problem ?? null} onScore={onScore} online={online} />

      <section className="stack gap-3" aria-labelledby="hear-runs-title">
        <h2 id="hear-runs-title" className="t-subheading">
          Runs
        </h2>
        {error && !runs ? (
          <Card>
            <EmptyState icon={<CloudOff />} title="Couldn’t load the runs" description={error} actions={<Button onClick={() => setAttempt((n) => n + 1)}>Try again</Button>} />
          </Card>
        ) : !runs ? (
          <Card padding="md" aria-busy="true" aria-label="Loading">
            <SkeletonText lines={3} />
          </Card>
        ) : runs.length === 0 ? (
          <Card>
            <EmptyState icon={<Gauge />} title="No runs yet" description="Score a session above. Each run reads every take through Ghost’s speech to text and keeps the numbers, so a later run can be compared with it." />
          </Card>
        ) : (
          <Card padding="none">
            <Table interactive>
              <thead>
                <tr>
                  <th>Run</th>
                  <th>Session</th>
                  <th>Channel</th>
                  <th className="is-num">Takes</th>
                  <th className="is-num">stage</th>
                  <th className="is-num">turn</th>
                  <th className="is-num">file</th>
                  <th>Note</th>
                  <th className="is-shrink">
                    <span className="sr-only">Actions</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {runs.map((r) => (
                  <RunRow key={r.id} run={r} online={online} onOpen={() => navigate(`/hear/runs/${encodeURIComponent(r.id)}`)} onDelete={() => void remove(r)} onNote={(n) => saveNote(r, n)} />
                ))}
              </tbody>
            </Table>
          </Card>
        )}
        <p className="t-small t-2">Word error rates over requests and replies. <span className="t-mono">stage</span> is what Ghost hears today, <span className="t-mono">turn</span> the whole turn read once, <span className="t-mono">file</span> the whole take read with no VAD.</p>
      </section>
      {dialog}
    </div>
  );
}

function ScoreCard({ sessions, initial, scoring, problem, onScore, online }: { sessions: HearSessionSummary[] | null; initial: string | null; scoring: HearView["scoring"]; problem: string | null; onScore: Props["onScore"]; online: boolean }) {
  const scorable = (sessions ?? []).filter((s) => s.recorded > 0);
  const [pick, setPick] = useState<string | null>(initial);
  const [channel, setChannel] = useState<Channel>("aec");
  const [busy, setBusy] = useState(false);
  const [failed, setFailed] = useState<string | null>(null);
  const chosen = pick && scorable.some((s) => s.name === pick) ? pick : (scorable[0]?.name ?? null);
  const hasRaw = scorable.find((s) => s.name === chosen)?.raw ?? 0;

  const go = async () => {
    if (!chosen) return;
    setBusy(true);
    setFailed(null);
    try {
      await onScore(chosen, channel);
    } catch (e) {
      setFailed(refusal(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Card padding="lg" className="hear-score">
      <div className="hear-score__head">
        <h2 className="t-heading">Score a session</h2>
        <p className="t-small t-2">A run holds the mic while it reads every take, so Ghost isn’t listening for its name until it ends.</p>
      </div>
      {sessions == null ? (
        <Skeleton height={32} width={320} />
      ) : scorable.length === 0 ? (
        <p className="t-body t-2">Record a session first: a run scores the takes in one.</p>
      ) : (
        <div className="hear-score__form">
          <Field label="Session" className="hear-score__field">
            <Select value={chosen ?? ""} onChange={(e) => setPick(e.target.value)} disabled={!!scoring}>
              {scorable.map((s) => (
                <option key={s.name} value={s.name}>
                  {s.name} · {s.recorded} takes
                </option>
              ))}
            </Select>
          </Field>
          <Field label="Channel" className="hear-score__field" help={channel === "raw" && !hasRaw ? "This session has no raw array takes." : undefined}>
            <SegmentedControl<Channel>
              options={[
                { value: "aec", label: "ghost.mic" },
                { value: "raw", label: "Raw array", disabled: !hasRaw },
              ]}
              value={channel}
              onChange={setChannel}
              disabled={!!scoring}
            />
          </Field>
          <Button variant="primary" leadingIcon={<Gauge />} loading={busy} disabled={!online || !!scoring || !chosen || (channel === "raw" && !hasRaw)} onClick={() => void go()} className="hear-score__go">
            Score session {chosen}
          </Button>
        </div>
      )}
      {scoring && (
        <div className="hear-score__progress" aria-live="polite">
          <Progress value={scoring.total ? (scoring.done / scoring.total) * 100 : undefined} label={`Scoring ${scoring.session}`} />
          <p className="t-small t-2 t-num">
            Scoring <span className="t-mono">{scoring.session}</span> on {CHANNEL_WORDS[scoring.channel] ?? scoring.channel}: {scoring.total ? `${scoring.done} of ${scoring.total} takes` : "starting…"}
          </p>
        </div>
      )}
      {(failed || (problem && !scoring)) && (
        <Notice tone="danger" title="The run didn’t finish" onDismiss={failed ? () => setFailed(null) : undefined}>
          {failed ?? problem}
        </Notice>
      )}
    </Card>
  );
}

function RunRow({ run, online, onOpen, onDelete, onNote }: { run: HearRunInfo; online: boolean; onOpen: () => void; onDelete: () => void; onNote: (note: string) => Promise<boolean> }) {
  const [editing, setEditing] = useState(false);
  const [text, setText] = useState(run.note ?? "");
  const [saving, setSaving] = useState(false);
  const save = async () => {
    setSaving(true);
    const ok = await onNote(text.trim());
    setSaving(false);
    if (ok) setEditing(false);
  };
  const onKey = (e: KeyboardEvent<HTMLInputElement>) => {
    if (e.key === "Enter") {
      e.preventDefault();
      void save();
    }
    if (e.key === "Escape") {
      e.preventDefault();
      setText(run.note ?? "");
      setEditing(false);
    }
  };
  const rowKey = (e: KeyboardEvent<HTMLTableRowElement>) => {
    if (e.target === e.currentTarget && (e.key === "Enter" || e.key === " ")) {
      e.preventDefault();
      onOpen();
    }
  };
  return (
    <tr tabIndex={0} onClick={editing ? undefined : onOpen} onKeyDown={rowKey}>
      <td className="is-mono is-nowrap">{run.id}</td>
      <td className="is-mono is-nowrap">{run.session}</td>
      <td className="is-nowrap">{CHANNEL_WORDS[run.channel] ?? run.channel}</td>
      <td className="is-num">{run.takes}</td>
      <td className="is-num">{pct(run.wer.stage)}</td>
      <td className="is-num">{pct(run.wer.turn)}</td>
      <td className="is-num">{pct(run.wer.file)}</td>
      <td className="hear-note" onClick={(e) => e.stopPropagation()}>
        {editing ? (
          <Input size="sm" autoFocus value={text} maxLength={500} placeholder="Main before the change" onChange={(e) => setText(e.target.value)} onKeyDown={onKey} onBlur={() => void save()} disabled={saving} aria-label={`Note for run ${run.id}`} />
        ) : (
          <button type="button" className="hear-note__btn" onClick={() => setEditing(true)} disabled={!online} aria-label={run.note ? `Edit the note for run ${run.id}: ${run.note}` : `Add a note to run ${run.id}`}>
            <span className={run.note ? "t-1" : "t-3"} data-user-content>
              {run.note || "Add a note"}
            </span>
            <Pencil aria-hidden />
          </button>
        )}
      </td>
      <td className="is-shrink" onClick={(e) => e.stopPropagation()}>
        <Menu
          align="end"
          trigger={<IconButton size="sm" variant="ghost" label={`More for run ${run.id}`} icon={<MoreHorizontal />} disabled={!online} />}
          items={[
            { label: "Open the report", onSelect: onOpen },
            { label: "Edit the note", icon: <Pencil />, onSelect: () => setEditing(true) },
            "separator",
            { label: `Delete ${run.id}`, icon: <Trash2 />, danger: true, onSelect: onDelete },
          ]}
        />
      </td>
    </tr>
  );
}

function RunDetail({ id, runs }: { id: string; runs: HearRunInfo[] | null }) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [baseline, setBaseline] = useState<string>("");
  const [run, setRun] = useState<HearRun | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);

  useEffect(() => {
    let on = true;
    setError(null);
    client
      .hearRun(id, baseline || null)
      .then((r) => on && setRun(r))
      .catch((e) => on && setError(refusal(e)));
    return () => {
      on = false;
    };
  }, [client, id, baseline, attempt]);

  const others = (runs ?? []).filter((r) => r.id !== id);
  const play = (take: string) => {
    if (!run) return;
    client.hearPlay(run.info.session, take, "aec").catch((e) => toast({ title: "Couldn’t play the take", description: refusal(e), tone: "danger" }));
  };

  return (
    <div className="stack gap-6 hear-run">
      <Link to="/hear/runs" className="hear-back t-small">
        <ChevronLeft aria-hidden /> Runs
      </Link>
      <div className="hear-run__head">
        <div>
          <h2 className="t-title">
            Run <span className="t-mono">{id}</span>
          </h2>
          {run && (
            <p className="t-small t-2">
              Session <span className="t-mono">{run.info.session}</span> · {CHANNEL_WORDS[run.info.channel] ?? run.info.channel} · {run.info.takes} takes · {run.info.created}
              {run.info.note && (
                <>
                  {" "}
                  · <span data-user-content>{run.info.note}</span>
                </>
              )}
            </p>
          )}
        </div>
        <Field label="Compare with" help="The baseline is the run you’re comparing against: usually the one before your change." className="hear-run__baseline">
          <Select value={baseline} onChange={(e) => setBaseline(e.target.value)} disabled={!others.length}>
            <option value="">No baseline</option>
            {others.map((r) => (
              <option key={r.id} value={r.id}>
                {r.id} · {r.session} · {CHANNEL_WORDS[r.channel] ?? r.channel}
                {r.note ? ` · ${r.note.slice(0, 40)}` : ""}
              </option>
            ))}
          </Select>
        </Field>
      </div>
      {error ? (
        <Notice tone="danger" title="Couldn’t load the report" action={<Button size="sm" onClick={() => setAttempt((n) => n + 1)}>Try again</Button>}>
          {error}
        </Notice>
      ) : !run ? (
        <Card padding="lg" aria-busy="true" aria-label="Loading">
          <SkeletonText lines={6} />
        </Card>
      ) : (
        <Card padding="lg">
          {run.report.pass == null && baseline === "" && others.length > 0 && <p className="t-small t-2 hear-run__hint">Pick a baseline to see what changed, take by take, and whether this run passes.</p>}
          <ReportView report={run.report} onPlay={play} canPlay={online} />
        </Card>
      )}
    </div>
  );
}
