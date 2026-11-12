/**
 * Today's telemetry (ADR 0060; the signal concept, docs/design/concepts-2026-11.md): the day's
 * true numbers in large type, and a fine margin of the latest calls as they come back. Every
 * figure is a count from the daemon (`GET /reach`, the `reach` events, the runs, the apps, the
 * last model check), never an estimate. Still when nothing is happening; alive while agents
 * work.
 */
import type { ReachCall } from "@ghost/client-ts";
import { useEffect, useState } from "react";

import { hrefFor } from "../../app/router";
import { nameOf, useToday } from "../../app/today";
import { cx, formatTime } from "../../ui";
import { bytes, count, ms, perMinute, wireOf } from "./telemetry";

export function Signal() {
  const today = useToday();
  const t = today.tally;
  const working = today.working.filter((r) => r.status === "running").length;
  const now = useNow(working > 0 || today.calls.length > 0 ? 5_000 : 0);
  const rate = perMinute(today.calls, now);
  const wire = wireOf(today.calls.length ? today.calls : [], 6);
  const appsTitle = t.apps.length ? t.apps.join(", ") : "none yet";

  return (
    <section className="today-signal" aria-label="How much Ghost read and did today">
      <ul className="today-figures" role="list">
        <Figure label="items read" value={count(t.items)} hint={t.calls ? `in ${count(t.calls)} ${t.calls === 1 ? "call" : "calls"}` : "no calls yet"} />
        <Figure label="read" value={bytes(t.bytes)} hint={t.failed ? `${count(t.failed)} failed` : t.calls ? "as the apps sent it" : "—"} trouble={t.failed > 0} />
        <Figure label="apps reached" value={`${t.apps.length}`} unit={`of ${today.reach.apps}`} hint={appsTitle} href="/settings/apps" trouble={today.reach.down > 0} troubleText={today.reach.down ? `${today.reach.down} not connected` : undefined} />
        <Figure label="changes made" value={count(today.counts.changes)} hint={`${count(today.counts.runs)} ${today.counts.runs === 1 ? "run" : "runs"} · ${count(today.counts.modelCalls)} model calls`} trouble={today.counts.failedChanges > 0} troubleText={today.counts.failedChanges ? `${today.counts.failedChanges} failed` : undefined} />
        <Figure label="working now" value={`${working}`} hint={working ? today.working.filter((r) => r.status === "running").map((r) => nameOf(today.agents, r.agent)).join(", ") : "nobody"} live={working > 0} />
        <Figure label="first word" value={today.speed ? today.speed.text : "—"} hint={today.speed ? today.speed.model : "not checked yet"} href="/settings/model" />
      </ul>

      <div className="today-wire" aria-label="The latest calls" role="group">
        <p className="today-wire__head">
          <span>calls</span>
          <span className={cx("today-wire__rate t-num", rate > 0 && "is-live")}>{rate > 0 ? `${rate} in the last minute` : "quiet"}</span>
        </p>
        {wire.length ? (
          <ol className="today-wire__list" aria-live="off">
            {wire.map((c, i) => (
              <WireLine key={`${c.at}-${c.session_id}-${c.tool}-${i}`} c={c} who={nameOf(today.agents, c.agent)} fresh={i === 0 && now - Date.parse(c.at) < 4_000} />
            ))}
          </ol>
        ) : (
          <p className="today-wire__empty">{t.calls ? `${count(t.calls)} today; the next one shows here as it comes back.` : "When an agent reads or changes something, each call shows here as it comes back."}</p>
        )}
      </div>
    </section>
  );
}

function WireLine({ c, who, fresh }: { c: ReachCall; who: string; fresh: boolean }) {
  return (
    <li className={cx("today-wire__line", fresh && "is-fresh", !c.ok && "is-failed", !c.read && "is-change")}>
      <span className="today-wire__time">{formatTime(c.at)}</span>
      <span className="today-wire__who">{who}</span>
      <span className="today-wire__tool">
        {c.app}.{c.tool}
      </span>
      <span className="today-wire__n">{c.ok ? (c.items ?? "–") : "failed"}</span>
      <span className="today-wire__size">{c.ok ? bytes(c.bytes) : ""}</span>
      <span className="today-wire__ms">{ms(c.ms)}</span>
    </li>
  );
}

function Figure({ label, value, unit, hint, href, live, trouble, troubleText }: { label: string; value: string; unit?: string; hint: string; href?: string; live?: boolean; trouble?: boolean; troubleText?: string }) {
  const inner = (
    <>
      <span className="today-figure__value t-num">
        {value}
        {unit && <span className="today-figure__unit"> {unit}</span>}
      </span>
      <span className="today-figure__label">
        {live && <span className="today-figure__live" aria-hidden />}
        {label}
      </span>
      <span className={cx("today-figure__hint", trouble && "is-trouble")}>{troubleText ? `${troubleText} · ${hint}` : hint}</span>
    </>
  );
  return (
    <li>
      {href ? (
        <a className="today-figure is-link" href={hrefFor(href)}>
          {inner}
        </a>
      ) : (
        <div className="today-figure">{inner}</div>
      )}
    </li>
  );
}

/** The time, refreshed every `every` ms while it is non-zero. */
function useNow(every: number): number {
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    setNow(Date.now());
    if (!every) return;
    const t = window.setInterval(() => setNow(Date.now()), every);
    return () => window.clearInterval(t);
  }, [every]);
  return now;
}
