/**
 * Owner: hear. Wake takes (ADR 0065): the sound of each time Ghost thought it heard its name,
 * kept on this computer to learn the operator's voice. Each is labelled with one key and
 * the next unlabelled one plays at once (`wakeTakes.ts` has the keys). Takes play through
 * Ghost's own speaker, as the Hearing test's do; no audio comes to this screen.
 */
import type { WakeTakeView, WakeTakesView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Play, Trash2 } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { Badge, Button, Card, EmptyState, IconButton, Kbd, Notice, Skeleton, Switch, Tooltip, cx, errorText, relativeTime, useConfirm, useToast } from "../../ui";
import { KEEP_NOTICE, LABEL_TEXT, afterDelete, bytesText, curvePath, keyAction, move, nextAfterLabel, outcomeText, tally } from "./wakeTakes";

const CURVE_W = 160;
const CURVE_H = 28;

function Curve({ t }: { t: WakeTakeView }) {
  const d = curvePath(t.scores, CURVE_W, CURVE_H);
  const y = CURVE_H - t.threshold * CURVE_H;
  return (
    <svg className="wt-curve" width={CURVE_W} height={CURVE_H} viewBox={`0 0 ${CURVE_W} ${CURVE_H}`} aria-label={`Best score ${Math.max(0, ...t.scores).toFixed(2)}`}>
      <line x1={0} x2={CURVE_W} y1={y} y2={y} className="wt-curve-threshold" />
      {d && <path d={d} className="wt-curve-line" />}
    </svg>
  );
}

export function WakeTakesTab() {
  const { client } = useGhost();
  const toast = useToast();
  const { confirm, dialog } = useConfirm();
  const [view, setView] = useState<WakeTakesView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [sel, setSel] = useState(0);
  const listRef = useRef<HTMLDivElement>(null);

  const load = useCallback(() => {
    client
      .wakeTakes()
      .then((v) => (setView(v), setError(null)))
      .catch((e) => setError(errorText(e)));
  }, [client]);
  useEffect(load, [load]);

  const takes = view?.takes ?? [];
  const current = sel >= 0 && sel < takes.length ? takes[sel] : null;

  const play = useCallback(
    (t: WakeTakeView | null, array = false) => {
      if (!t) return;
      client.playWakeTake(t.id, array).catch((e) => toast({ tone: "danger", title: "Couldn’t play it", description: errorText(e) }));
    },
    [client, toast],
  );

  const label = async (t: WakeTakeView, l: WakeTakeView["label"]) => {
    try {
      const got = await client.labelWakeTake(t.id, l);
      const list = takes.map((k) => (k.id === got.id ? got : k));
      setView((v) => (v ? { ...v, takes: list } : v));
      if (l) {
        const next = nextAfterLabel(list, sel);
        setSel(next);
        if (next !== sel && next >= 0 && !list[next].label) play(list[next]);
      }
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t label it", description: errorText(e) });
    }
  };

  const remove = async (t: WakeTakeView) => {
    try {
      await client.deleteWakeTake(t.id);
      setSel(afterDelete(takes.length, sel));
      load();
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t delete it", description: errorText(e) });
    }
  };

  const removeAll = async () => {
    const ok = await confirm({
      title: "Delete every wake take?",
      description: `${view?.count ?? 0} takes and their sound go from this computer. This can’t be undone.`,
      confirmLabel: "Delete all",
      tone: "danger",
    });
    if (!ok) return;
    try {
      const r = await client.deleteWakeTakes();
      toast({ tone: "ok", title: `Deleted ${r.deleted} take${r.deleted === 1 ? "" : "s"}` });
      setSel(0);
      load();
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t delete them", description: errorText(e) });
    }
  };

  const setKeep = async (keep: boolean) => {
    if (!view) return;
    try {
      setView(await client.setWakeTakesChoice({ ...view.choice, keep }));
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t change it", description: errorText(e) });
    }
  };

  const setUnlabelled = async (on: boolean) => {
    if (!view) return;
    try {
      setView(await client.setWakeTakesChoice({ ...view.choice, unlabelled_accepted_are_me: on }));
    } catch (e) {
      toast({ tone: "danger", title: "Couldn’t change it", description: errorText(e) });
    }
  };

  const onKey = (e: React.KeyboardEvent) => {
    const a = keyAction(e);
    if (!a || !current) return;
    e.preventDefault();
    if (a.kind === "label") void label(current, a.label);
    else if (a.kind === "play") play(current, a.array);
    else if (a.kind === "move") setSel(move(takes.length, sel, a.by));
    else if (a.kind === "delete") void remove(current);
  };

  useEffect(() => {
    listRef.current?.querySelector<HTMLElement>(`[data-index="${sel}"]`)?.scrollIntoView({ block: "nearest" });
  }, [sel]);

  if (error && !view) {
    return (
      <Notice tone="danger" title="Couldn’t read the wake takes" action={<Button size="sm" onClick={load}>Try again</Button>}>
        {error}
      </Notice>
    );
  }
  if (!view) return <Skeleton height={240} />;

  const t = tally(takes, view.choice.unlabelled_accepted_are_me ?? false);
  return (
    <div className="wt">
      {dialog}
      <Card className="wt-head">
        <div className="wt-keep">
          <Switch checked={view.choice.keep} onChange={(on) => void setKeep(on)} label="Keep wake takes" />
          <p className="wt-notice">
            {KEEP_NOTICE} Nothing is sent anywhere. {view.count} kept, {bytesText(view.bytes)} of {bytesText(view.max_bytes)}; the oldest go first.
          </p>
        </div>
        <div className="wt-actions">
          <Switch checked={view.choice.unlabelled_accepted_are_me ?? false} onChange={(on) => void setUnlabelled(on)} label="Count unlabelled wakes as me" />
          <Button variant="danger" size="sm" leadingIcon={<Trash2 size={14} />} onClick={() => void removeAll()} disabled={!view.count}>
            Delete all
          </Button>
        </div>
      </Card>

      {takes.length === 0 ? (
        <EmptyState
          title="No wake takes yet"
          description={view.choice.keep ? "Each time Ghost hears something like its name, the sound is kept here for you to label." : "Keeping is off: turn it on to collect takes."}
        />
      ) : (
        <>
          <p className="wt-keys">
            <Kbd>Y</Kbd> me calling Ghost · <Kbd>N</Kbd> not a wake · <Kbd>B</Kbd> me, after a miss · <Kbd>Space</Kbd> play · <Kbd>A</Kbd> the array · <Kbd>↓</Kbd>
            <Kbd>↑</Kbd> move · <Kbd>Del</Kbd> delete. {t.unlabelled} to label; the retraining will use {t.positives} of you and {t.negatives} not.
          </p>
          <div className="wt-list" ref={listRef} tabIndex={0} role="listbox" aria-label="Wake takes" aria-activedescendant={current ? `wt-${current.id}` : undefined} onKeyDown={onKey}>
            {takes.map((k, i) => (
              <div
                key={k.id}
                id={`wt-${k.id}`}
                data-index={i}
                role="option"
                aria-selected={i === sel}
                className={cx("wt-row", i === sel && "is-selected")}
                onClick={() => setSel(i)}
                onDoubleClick={() => play(k)}
              >
                <Tooltip content="Play through Ghost’s speaker">
                  <IconButton size="sm" label="Play" icon={<Play size={14} />} onClick={() => play(k)} />
                </Tooltip>
                <span className="wt-when">{relativeTime(k.at)}</span>
                <Badge tone={k.outcome === "accepted" ? "ok" : "neutral"} size="sm">
                  {outcomeText(k)}
                </Badge>
                <span className="wt-score">{k.score.toFixed(2)}</span>
                <Curve t={k} />
                <span className="wt-reading">{k.reading ? `“${k.reading}”` : ""}</span>
                <span className="wt-input">{k.array ? `${k.input} + array` : k.input}</span>
                {k.label ? (
                  <Badge tone={k.label === "not" ? "warn" : "info"} size="sm">
                    {LABEL_TEXT[k.label]}
                  </Badge>
                ) : (
                  <span className="wt-unlabelled">Unlabelled</span>
                )}
                <IconButton size="sm" label="Delete" icon={<Trash2 size={14} />} onClick={() => void remove(k)} />
              </div>
            ))}
          </div>
        </>
      )}
    </div>
  );
}
