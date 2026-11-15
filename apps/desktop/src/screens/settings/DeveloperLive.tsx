/**
 * Owner: settings. Developer's live views (ADR 0073, amended 2026-11-14):
 *   Live timing overlay   a switch for the overlay (`app/TimingOverlay.tsx`) and Copy the last
 *                         ten turns as a plain-text table, numbers only
 *   Microphone meter      #/settings/developer/live, full width: the input level over the last
 *                         seconds, the room gate's levels (ADR 0072), the speech detector's
 *                         state, and what Ghost decided about each segment, by kind only
 *
 * The meter holds the daemon's meter open while this page is shown (`PUT /developer/meter`,
 * asked again every few seconds) and closes it when the page goes or is hidden. Levels and
 * kinds only: never the words, never audio, never a transcript, in a session or out of one.
 * Nothing it shows is kept once the page closes.
 *
 * States by URL (mock): `?meter=quiet|talker|echo` picks the synthetic room the mock meter plays.
 */
import type { MeterDecisionView, MeterView } from "@ghost/client-ts";
import { decisionTone, decisionWords, timingTable, useGhost } from "@ghost/ui";
import { Activity, Copy, Mic } from "lucide-react";
import { useEffect, useRef, useState, useSyncExternalStore } from "react";

import { setTimingOverlay, useTimingOverlay } from "../../app/developer";
import { notListening } from "../../app/listening";
import { hrefFor } from "../../app/router";
import { Badge, Button, Card, ListRow, Notice, Section, Switch, cx, useCopy } from "../../ui";
import { SECTION_TITLE, sectionDomId } from "./model";

/** The meter asks the daemon again this often; the daemon's lease is ten seconds. */
const RENEW_MS = 4000;
/** Seconds of level the meter draws. */
export const HISTORY_SECS = 8;
/** Decisions the list keeps while the page is open. */
const DECISIONS_SHOWN = 40;
/** The level scale the meter draws, dBFS. */
export const FLOOR_DB = -70;

/* ------------------------------------------------------------------ on Developer */

/** Developer's Live views section: the overlay's switch and Copy, and the way to the meter. */
export function LiveViews() {
  const { dev } = useGhost();
  const on = useTimingOverlay();
  const timings = useSyncExternalStore(dev.subscribe, dev.timings, dev.timings);
  const [copied, copy] = useCopy();
  return (
    <Section
      id={sectionDomId("live")}
      title={SECTION_TITLE.live}
      description="What Ghost is doing as it happens, for testing a build. Numbers and kinds only: never what was said."
      className="set-section"
    >
      <Card list>
        <ListRow
          leading={<Activity size={16} aria-hidden />}
          title="Live timing overlay"
          description="Each turn’s stages as it happens, in small monospace under Today’s live strip, the conversation and the floating Ghost: your turn closing, speech to text, the model’s first word, the first tool call and its result, Ghost’s first sound, the reply’s end, and which model answered."
          trailing={<Switch aria-label="Live timing overlay" checked={on} onChange={(v: boolean) => setTimingOverlay(v)} />}
        />
        <ListRow
          title="Copy the last ten turns"
          description={
            timings.length
              ? `A plain-text table of ${timings.length} turn${timings.length === 1 ? "" : "s"}' timings, to paste into an issue. A diagnostic report on About carries the last 30 as well.`
              : "Turns are timed while Ghost is open. Talk to Ghost, then copy them here to paste into an issue."
          }
          trailing={
            <Button size="sm" variant="secondary" leadingIcon={<Copy />} disabled={!timings.length} onClick={() => copy(timingTable(timings.slice(-10)))}>
              {copied ? "Copied" : "Copy"}
            </Button>
          }
        />
        <ListRow
          leading={<Mic size={16} aria-hidden />}
          title="Microphone meter"
          description="What Ghost’s microphone hears, as a level over the last seconds, the room’s level and yours as Ghost tells you apart, and what Ghost decided about each stretch of sound."
          href={hrefFor("/settings/developer/live")}
          chevron
        />
      </Card>
    </Section>
  );
}

/* ------------------------------------------------------------------ the meter */

interface Point {
  level: number | null;
  peak: number | null;
  speaking: boolean;
}

/** One reading's height on the meter's scale, 0 at `FLOOR_DB`, 1 at 0 dBFS. */
export function heightOf(db: number | null | undefined): number {
  if (typeof db !== "number" || !Number.isFinite(db)) return 0;
  return Math.max(0, Math.min(1, (db - FLOOR_DB) / -FLOOR_DB));
}

/** Holds the daemon's meter open while the page is shown, and closes it after. */
function useMeterLease(enabled: boolean) {
  const { client } = useGhost();
  const [error, setError] = useState<string | null>(null);
  useEffect(() => {
    if (!enabled) return;
    let alive = true;
    const ask = () => {
      if (document.visibilityState === "hidden") return;
      client
        .setMeter(true)
        .then(() => alive && setError(null))
        .catch((e: unknown) => alive && setError(e instanceof Error ? e.message : String(e)));
    };
    const onVisible = () => {
      if (document.visibilityState === "hidden") void client.setMeter(false).catch(() => {});
      else ask();
    };
    ask();
    const t = window.setInterval(ask, RENEW_MS);
    document.addEventListener("visibilitychange", onVisible);
    return () => {
      alive = false;
      window.clearInterval(t);
      document.removeEventListener("visibilitychange", onVisible);
      void client.setMeter(false).catch(() => {});
    };
  }, [client, enabled]);
  return error;
}

const STATE_WORDS: Record<MeterView["state"], string> = {
  listening_off: "Listening is off",
  passive: "Waiting for its name",
  session: "In a session",
  quiet: "Nothing is listening right now",
};

const GATE_WORDS: Record<MeterView["room_gate"]["why"], string> = {
  this_microphone: "On: this microphone hears the room nearly as loud as you, so Ghost holds back speech far under your level unless it says Ghost’s name.",
  on_chip_array: "Off: the on-chip array (the reSpeaker XVF3800) aims at the talker itself, and its gain control sits between you and Ghost.",
  setting: "Set by GHOST_ROOM_GATE.",
  no_session: "It runs in a desk session only.",
};

function db(x: number | null | undefined): string {
  return typeof x === "number" ? `${x.toFixed(0)} dBFS` : "–";
}

function Trace({ points, room }: { points: Point[]; room: MeterView["room_gate"] | null }) {
  const w = 600;
  const h = 120;
  const n = Math.max(points.length, 2);
  const x = (i: number) => (i / (n - 1)) * w;
  const y = (v: number) => h - v * h;
  const area = points.length ? `M0,${h} ${points.map((p, i) => `L${x(i).toFixed(1)},${y(heightOf(p.level)).toFixed(1)}`).join(" ")} L${x(points.length - 1).toFixed(1)},${h} Z` : "";
  const peaks = points.map((p, i) => `${i ? "L" : "M"}${x(i).toFixed(1)},${y(heightOf(p.peak)).toFixed(1)}`).join(" ");
  const lines = room?.on
    ? ([
        [room.room, "meter-trace__room", "The room"],
        [room.person, "meter-trace__person", "You"],
        [room.line, "meter-trace__line", "Held under"],
      ] as const)
    : [];
  return (
    <>
      <svg className="meter-trace" viewBox={`0 0 ${w} ${h}`} preserveAspectRatio="none" role="img" aria-label="The input level over the last seconds">
        {points.map((p, i) => (p.speaking ? <rect key={i} className="meter-trace__speech" x={x(i)} y={0} width={w / n + 1} height={h} /> : null))}
        {area && <path className="meter-trace__level" d={area} />}
        {points.length > 1 && <path className="meter-trace__peak" d={peaks} />}
        {lines.map(([v, cls]) => (typeof v === "number" ? <line key={cls} className={cls} x1={0} x2={w} y1={y(heightOf(v))} y2={y(heightOf(v))} /> : null))}
      </svg>
      {lines.some(([v]) => typeof v === "number") && (
        <p className="meter-legend">
          {lines.map(([v, cls, label]) =>
            typeof v === "number" ? (
              <span key={cls} className={cls}>
                <span className="meter-legend__swatch" aria-hidden /> {label} {v.toFixed(0)} dBFS
              </span>
            ) : null,
          )}
        </p>
      )}
    </>
  );
}

function DecisionRow({ d, at }: { d: MeterDecisionView; at: number }) {
  return (
    <li className={cx("meter-decision", `is-${decisionTone(d)}`)}>
      <span className="meter-decision__at">{new Date(at).toLocaleTimeString([], { hour: "2-digit", minute: "2-digit", second: "2-digit" })}</span>
      <span>{decisionWords(d)}</span>
    </li>
  );
}

/** #/settings/developer/live: the microphone meter, full width. */
export function DeveloperLivePage() {
  const { dev, status, connected } = useGhost();
  const off = !!notListening(status);
  const error = useMeterLease(connected);
  const [now, setNow] = useState<MeterView | null>(null);
  const [points, setPoints] = useState<Point[]>([]);
  const [decisions, setDecisions] = useState<{ d: MeterDecisionView; at: number; k: number }[]>([]);
  const seq = useRef(0);
  useEffect(
    () =>
      dev.subscribeMeter((m) => {
        if (!m.open) {
          setNow(null);
          return;
        }
        setNow(m);
        setPoints((p) => [...p, { level: m.level ?? null, peak: m.peak ?? null, speaking: m.speaking }].slice(-HISTORY_SECS * 15));
        if (m.decisions.length) {
          const t = Date.now();
          setDecisions((list) => [...m.decisions.map((d) => ({ d, at: t - d.ago_ms, k: seq.current++ })).reverse(), ...list].slice(0, DECISIONS_SHOWN));
        }
      }),
    [dev],
  );
  const state = off ? "listening_off" : (now?.state ?? null);
  const gate = now?.room_gate ?? null;
  return (
    <>
      <Notice tone="info" title="Levels and decisions, never words">
        This page shows how loud Ghost’s microphone is and what Ghost decides about each stretch of sound, about fifteen times a second while it is open. It never shows or keeps what was said or any audio, and it opens no microphone: it reads what Ghost’s listening already measures. Close the page and it stops.
      </Notice>
      {error && (
        <Notice tone="danger" title="Couldn’t open the meter">
          {error}
        </Notice>
      )}
      {off && (
        <Notice tone="warn" title="Listening is off">
          No microphone is open, so there is nothing to measure. Turn listening on to see the meter move.
        </Notice>
      )}
      <Section id="set-meter" title="Microphone" description={state ? STATE_WORDS[state] : connected ? "Opening the meter…" : "Ghost isn’t answering."} className="set-section">
        <Card>
          <div className="meter">
            <div className="meter-head">
              <Badge size="sm" tone={state === "session" ? "ok" : state === "listening_off" ? "warn" : "neutral"} dot>
                {state ? STATE_WORDS[state] : "Opening"}
              </Badge>
              <span className="meter-num">level {db(now?.level)}</span>
              <span className="meter-num">peak {db(now?.peak)}</span>
              <Badge size="sm" tone={now?.speaking ? "accent" : "neutral"}>
                {now?.speaking ? "Speech" : "No speech"}
              </Badge>
            </div>
            <Trace points={points} room={gate} />
            <p className="meter-scale">
              The last {HISTORY_SECS} seconds, {FLOOR_DB} to 0 dBFS. Shaded: the speech detector heard speech.
            </p>
          </div>
        </Card>
      </Section>
      <Section id="set-room" title="The person or the room" description="How Ghost tells you from the room by level (ADR 0072)." className="set-section">
        <Card list>
          <ListRow title="Room gate" description={gate ? GATE_WORDS[gate.why] : "Known once the meter is open."} meta={gate ? <Badge size="sm" tone={gate.on ? "ok" : "neutral"}>{gate.on ? "On" : "Off"}</Badge> : undefined} />
          {gate?.on && state === "session" && (
            <>
              <ListRow title="The room" description="Its loud level, outside your speech, once five seconds were heard." meta={<span className="set-value is-mono">{db(gate.room)}</span>} />
              <ListRow title="You" description="Your level, from the segment that woke Ghost and your turns since." meta={<span className="set-value is-mono">{db(gate.person)}</span>} />
              <ListRow title="Held under" description="Speech under this is held as the room, unless it begins with Ghost’s name." meta={<span className="set-value is-mono">{db(gate.line)}</span>} />
            </>
          )}
        </Card>
      </Section>
      <Section id="set-decisions" title="What Ghost decided" description="Each stretch of sound, newest first, by kind: never its words." className="set-section">
        <Card>
          {decisions.length ? (
            <ul className="meter-decisions" aria-live="off">
              {decisions.map(({ d, at, k }) => (
                <DecisionRow key={k} d={d} at={at} />
              ))}
            </ul>
          ) : (
            <p className="meter-empty">{off ? "Nothing, with listening off." : "Nothing yet. Say something, or Ghost’s name."}</p>
          )}
        </Card>
      </Section>
    </>
  );
}
