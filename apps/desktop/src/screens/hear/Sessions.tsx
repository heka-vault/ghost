/**
 * Owner: hear. Sessions: every session recorded, newest first (GET /hear/sessions). Continue
 * opens it in Record, Score takes it to Runs, Delete removes it and its audio. A session opens
 * to its takes, each with its level, how it ended, its warnings and a way to play it through
 * Ghost's speaker (ghost.mic, and the raw array where there is one).
 */
import type { HearSession, HearSessionSummary, HearView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ChevronRight, CloudOff, Mic, MoreHorizontal, Play, Square, Trash2 } from "lucide-react";
import { Fragment, type KeyboardEvent, useEffect, useState } from "react";

import { Badge, Button, Card, EmptyState, IconButton, Menu, Notice, Skeleton, Table, Tooltip, cx, errorText, relativeTime, useConfirm, useLive, useToast } from "../../ui";
import { STATUS_WORDS, dbfs, refusal, secs, takeStatus } from "./model";

const ENDING_WORDS: Record<string, string> = { hold: "Held", quiet: "On silence", timed: "Timed" };
const WARNING_WORDS: Record<string, string> = {
  too_quiet: "Too quiet",
  clipping: "Clipping",
  no_speech: "No speech",
  speech_in_silence: "Speech in silence",
  started_early: "Started early",
  cut_off: "Cut off",
  missed_end: "Missed the end",
};

interface Props {
  view: HearView | null;
  onContinue: (name: string) => void;
  onScore: (name: string) => void;
}

export function SessionsTab({ view, onContinue, onScore }: Props) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [list, setList] = useState<HearSessionSummary[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [openName, setOpenName] = useState<string | null>(null);
  const [detail, setDetail] = useState<Record<string, HearSession | "error">>({});

  useEffect(() => {
    let on = true;
    client
      .hearSessions()
      .then((l) => on && (setList(l), setError(null)))
      .catch((e) => on && setError(errorText(e)));
    return () => {
      on = false;
    };
  }, [client, attempt]);

  const toggle = (name: string) => {
    const next = openName === name ? null : name;
    setOpenName(next);
    if (next && !detail[next]) {
      client
        .hearSession(next)
        .then((s) => setDetail((d) => ({ ...d, [next]: s })))
        .catch(() => setDetail((d) => ({ ...d, [next]: "error" })));
    }
  };
  const onRowKey = (e: KeyboardEvent, name: string) => {
    if (e.target !== e.currentTarget) return;
    if (e.key === "Enter" || e.key === " ") {
      e.preventDefault();
      toggle(name);
    }
  };

  const remove = async (s: HearSessionSummary) => {
    const ok = await confirm({
      title: `Delete session ${s.name}?`,
      description: `Its ${s.recorded} take${s.recorded === 1 ? "" : "s"} and their audio go from this computer. Runs already scored from it stay.`,
      confirmLabel: `Delete ${s.name}`,
      tone: "danger",
    });
    if (!ok) return;
    try {
      await client.deleteHearSession(s.name);
      setList((l) => l?.filter((x) => x.name !== s.name) ?? null);
      toast({ title: "Deleted", description: `Session ${s.name} and its audio are gone.` });
    } catch (e) {
      toast({ title: "Couldn’t delete the session", description: refusal(e), tone: "danger" });
    }
  };

  const [playing, setPlaying] = useState<string | null>(null);
  const play = async (session: string, take: string, channel: "aec" | "raw") => {
    try {
      setPlaying(`${session}/${take}/${channel}`);
      await client.hearPlay(session, take, channel);
    } catch (e) {
      setPlaying(null);
      toast({ title: "Couldn’t play the take", description: refusal(e), tone: "danger" });
    }
  };
  // The daemon says when playback ends; the button follows it.
  useEffect(() => {
    if (!view?.playing) setPlaying(null);
  }, [view?.playing]);
  const takeGoing = !!view && ["countdown", "recording", "stopping", "checking"].includes(view.state);

  if (error && !list) {
    return (
      <Card>
        <EmptyState icon={<CloudOff />} title="Couldn’t load the sessions" description={error} actions={<Button onClick={() => setAttempt((n) => n + 1)}>Try again</Button>} />
      </Card>
    );
  }
  if (!list) {
    return (
      <Card list aria-busy="true" aria-label="Loading">
        {[0, 1, 2].map((i) => (
          <div key={i} className="hear-skel-row">
            <Skeleton width={96} height={12} />
            <Skeleton width={i ? 180 : 240} height={10} />
          </div>
        ))}
      </Card>
    );
  }
  if (list.length === 0) {
    return (
      <Card>
        <EmptyState
          icon={<Mic />}
          title="No sessions yet"
          description="A session is one sitting of takes. Open Record and hold Space while you read the first line; the session is kept from its first take."
          actions={<Button onClick={() => onContinue("")}>Start recording</Button>}
        />
      </Card>
    );
  }

  return (
    <>
      {error && (
        <Notice tone="danger" title="Couldn’t refresh the sessions" onDismiss={() => setError(null)}>
          {error} This is the list as it was.
        </Notice>
      )}
      <Card padding="none">
        <Table interactive>
          <thead>
            <tr>
              <th>Session</th>
              <th className="is-num">Recorded</th>
              <th className="is-num">Skipped</th>
              <th className="is-num">Worth a redo</th>
              <th className="is-num">Minutes</th>
              <th className="is-num">Raw array</th>
              <th>Updated</th>
              <th className="is-shrink">
                <span className="sr-only">Actions</span>
              </th>
            </tr>
          </thead>
          <tbody>
            {list.map((s) => {
              const isOpen = openName === s.name;
              const inRecorder = view?.session === s.name && view.state !== "closed";
              const d = detail[s.name];
              return (
                <Fragment key={s.name}>
                  <tr tabIndex={0} aria-expanded={isOpen} onClick={() => toggle(s.name)} onKeyDown={(e) => onRowKey(e, s.name)} className={cx("hear-session-row", isOpen && "is-open")}>
                    <td className="is-nowrap">
                      <span className="hear-session-name">
                        <ChevronRight aria-hidden className="hear-chevron" />
                        <span className="t-mono">{s.name}</span>
                        {inRecorder && (
                          <Badge size="sm" tone="info" dot>
                            In the recorder
                          </Badge>
                        )}
                      </span>
                    </td>
                    <td className="is-num">{s.recorded}</td>
                    <td className="is-num">{s.skipped}</td>
                    <td className="is-num">{s.flagged ? <span className="t-warn">{s.flagged}</span> : 0}</td>
                    <td className="is-num">{(s.seconds / 60).toFixed(1)}</td>
                    <td className="is-num">{s.raw}</td>
                    <td className="is-nowrap is-muted">{s.updated ? relativeTime(s.updated) : "Not yet"}</td>
                    <td className="is-shrink" onClick={(e) => e.stopPropagation()}>
                      <div className="hear-row-actions">
                        <Button size="sm" variant="secondary" onClick={() => onContinue(s.name)} disabled={!online}>
                          Continue
                        </Button>
                        <Button size="sm" variant="ghost" onClick={() => onScore(s.name)} disabled={!online || s.recorded === 0}>
                          Score
                        </Button>
                        <Menu
                          align="end"
                          trigger={<IconButton size="sm" variant="ghost" label={`More for session ${s.name}`} icon={<MoreHorizontal />} disabled={!online} />}
                          items={[{ label: `Delete ${s.name}`, icon: <Trash2 />, danger: true, disabled: inRecorder, reason: inRecorder ? "Open in the recorder" : undefined, onSelect: () => void remove(s) }]}
                        />
                      </div>
                    </td>
                  </tr>
                  {isOpen && (
                    <tr className="hear-session-detail">
                      <td colSpan={8}>
                        {d === "error" ? (
                          <p className="t-small t-danger">Couldn’t load this session’s takes.</p>
                        ) : !d ? (
                          <div className="stack gap-2" aria-busy="true">
                            <Skeleton width="60%" height={10} />
                            <Skeleton width="40%" height={10} />
                          </div>
                        ) : d.takes.length === 0 ? (
                          <p className="t-small t-2">No takes in this session yet.</p>
                        ) : (
                          <Table density="compact" className="hear-takes-table">
                            <thead>
                              <tr>
                                <th>Take</th>
                                <th>Line</th>
                                <th>State</th>
                                <th>Ended</th>
                                <th className="is-num">Peak</th>
                                <th className="is-num">Length</th>
                                <th>Warnings</th>
                                <th className="is-shrink">
                                  <span className="sr-only">Play</span>
                                </th>
                              </tr>
                            </thead>
                            <tbody>
                              {d.takes.map((t) => {
                                const st = takeStatus(t);
                                const key = (ch: string) => `${s.name}/${t.id}/${ch}`;
                                return (
                                  <tr key={t.id}>
                                    <td className="is-mono is-nowrap">{t.id}</td>
                                    <td className="hear-takes-table__line" data-user-content>
                                      {t.text || <span className="t-2">Silent</span>}
                                    </td>
                                    <td className="is-nowrap">
                                      <Badge size="sm" dot tone={st === "flagged" ? "warn" : st === "recorded" ? "ok" : "neutral"}>
                                        {STATUS_WORDS[st]}
                                      </Badge>
                                    </td>
                                    <td className="is-nowrap is-muted">{t.ending ? (ENDING_WORDS[t.ending] ?? t.ending) : "–"}</td>
                                    <td className="is-num is-nowrap">{dbfs(t.peak)}</td>
                                    <td className="is-num is-nowrap">{secs(t.seconds)}</td>
                                    <td className="hear-takes-table__warn t-small">{(t.warnings ?? []).map((w) => WARNING_WORDS[w] ?? w).join(", ") || <span className="t-3">None</span>}</td>
                                    <td className="is-shrink">
                                      {t.state === "recorded" && (
                                        <div className="hear-row-actions">
                                          {(["aec", "raw"] as const)
                                            .filter((ch) => ch === "aec" || t.raw)
                                            .map((ch) =>
                                              playing === key(ch) ? (
                                                <IconButton key={ch} size="sm" variant="ghost" label="Stop playing" icon={<Square />} onClick={() => void client.hearStopPlaying().catch(() => {})} />
                                              ) : (
                                                <Tooltip key={ch} content={ch === "aec" ? `Play ${t.id} (ghost.mic)` : `Play ${t.id} (the raw array)`}>
                                                  <Button size="sm" variant="ghost" leadingIcon={<Play />} disabled={!online || takeGoing} onClick={() => void play(s.name, t.id, ch)}>
                                                    {ch === "aec" ? "Play" : "Raw"}
                                                  </Button>
                                                </Tooltip>
                                              ),
                                            )}
                                        </div>
                                      )}
                                    </td>
                                  </tr>
                                );
                              })}
                            </tbody>
                          </Table>
                        )}
                      </td>
                    </tr>
                  )}
                </Fragment>
              );
            })}
          </tbody>
        </Table>
      </Card>
      <p className="t-small t-2 hear-foot">Takes are kept as 16 kHz audio under <span className="t-mono">~/.local/share/ghost/hear/</span> on this computer. What Ghost heard of each is never kept.</p>
      {dialog}
    </>
  );
}
