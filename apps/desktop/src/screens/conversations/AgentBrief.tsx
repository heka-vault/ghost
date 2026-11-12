/**
 * The head of an agent's page: the agent as someone with a job. What it is doing now (a run
 * with its live clock and what it is touching, or idle since when), what it did today (its
 * own slice of Today's timeline), when it works on its own (its schedules in words), how much
 * it read today (ADR 0060), and its voice, persona, memory and reach, each one click from its
 * settings. Every word and number comes from records Ghost keeps; nothing is estimated.
 */
import type { ConversationDetail } from "@ghost/client-ts";
import { Brain, CalendarClock, ShieldCheck, SlidersHorizontal } from "lucide-react";
import { useEffect, useState } from "react";

import { Link } from "../../app/router";
import { useToday } from "../../app/today";
import { Button, cx, formatTime, relativeTime } from "../../ui";
import { clock, type Moment } from "../today/model";
import { bytes, count, reachSentence } from "../today/telemetry";
import { whenWords } from "../watching/model";
import { briefOf } from "./brief";

const VERB: Record<Moment["kind"], string> = { change: "changed", run: "ran", watch: "found", question: "asked", answer: "was answered", note: "noted" };
/** Its day as shown here before “and N more”. */
const TODAY_SHOWN = 6;

export function AgentBrief({ slug, name, detail, voiceTitle, onPersona, onMemory }: { slug: string; name: string; detail: ConversationDetail; voiceTitle: string; onPersona: () => void; onMemory: () => void }) {
  const today = useToday();
  const b = briefOf(slug, today);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!b.running) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [b.running]);
  const persona = (detail.persona ?? "").trim().split(/\n+/)[0] ?? "";
  const notes = detail.notes?.length ?? 0;
  const zone = Intl.DateTimeFormat().resolvedOptions().timeZone;
  const reachApps = today.reach.apps;

  return (
    <section className="conv-brief" aria-label={`${name} at a glance`}>
      <div className={cx("conv-brief__now", b.running && "is-working")}>
        {b.running ? (
          <>
            <span className="conv-brief__pulse" aria-hidden />
            <p className="conv-brief__line">
              Running <span data-user-content>{b.running.name}</span>
              <span className="conv-brief__clock t-num" aria-label={`for ${clock(b.running.started_at ?? b.running.scheduled_for, now)}`}>
                {b.running.status === "queued" ? "queued" : clock(b.running.started_at ?? b.running.scheduled_for, now)}
              </span>
            </p>
            {b.touching && (
              <p className="conv-brief__touch">
                now <code>{b.touching.app}.{b.touching.tool}</code>
              </p>
            )}
          </>
        ) : (
          <p className="conv-brief__line is-idle">{detail.last_active_at ? `Idle · last at work ${relativeTime(detail.last_active_at)}` : "Idle · hasn’t worked yet"}</p>
        )}
      </div>

      <dl className="conv-brief__figures">
        <div>
          <dt>items read today</dt>
          <dd className="t-num">{count(b.reach.items)}</dd>
        </div>
        <div>
          <dt>read</dt>
          <dd className="t-num">{bytes(b.reach.bytes)}</dd>
        </div>
        <div>
          <dt>calls</dt>
          <dd className="t-num">{count(b.reach.calls)}</dd>
        </div>
        <div>
          <dt>did today</dt>
          <dd className="t-num">{count(b.today.length)}</dd>
        </div>
      </dl>
      <p className="conv-brief__sentence">{b.reach.calls ? `${reachSentence(b.reach)} today${b.reach.apps.length ? `: ${b.reach.apps.join(", ")}` : ""}.` : "It hasn’t read anything today."}</p>

      <div className="conv-brief__grid">
        <section className="conv-brief__part" aria-labelledby={`brief-day-${slug}`}>
          <h2 id={`brief-day-${slug}`} className="conv-brief__title">
            Its day
          </h2>
          {b.today.length ? (
            <ol className="conv-brief__day">
              {b.today.slice(0, TODAY_SHOWN).map((m) => (
                <li key={m.id} className={cx("conv-brief__moment", `is-${m.kind}`, `tone-${m.tone}`)}>
                  <span className="conv-brief__time">{formatTime(m.at)}</span>
                  <span className="conv-brief__mark" aria-hidden />
                  <span className="conv-brief__words">
                    <span className="conv-brief__verb">{m.tone === "failed" ? "couldn’t finish" : m.tone === "running" ? "is running" : VERB[m.kind]}</span> <span data-user-content>{m.title}</span>
                  </span>
                </li>
              ))}
            </ol>
          ) : (
            <p className="conv-brief__empty">Nothing yet today. What it does on its own, and what it changes, shows here.</p>
          )}
          {b.today.length > TODAY_SHOWN && <p className="conv-brief__more">and {b.today.length - TODAY_SHOWN} more on Today</p>}
        </section>

        <section className="conv-brief__part" aria-labelledby={`brief-when-${slug}`}>
          <h2 id={`brief-when-${slug}`} className="conv-brief__title">
            On its own
          </h2>
          {b.schedules.length ? (
            <ul className="conv-brief__list">
              {b.schedules.map((s) => (
                <li key={s.id}>
                  <CalendarClock aria-hidden />
                  <span>
                    <span className="conv-brief__strong" data-user-content>
                      {s.name}
                    </span>{" "}
                    · {whenWords(s, zone).text}
                    {!s.enabled && " · paused"}
                  </span>
                </li>
              ))}
            </ul>
          ) : (
            <p className="conv-brief__empty">No schedule. Tell it {"“"}check this every morning{"”"} in a session, or add one on Watching.</p>
          )}
          <Link to="/watching" className="conv-brief__link">
            Schedules on Watching
          </Link>
        </section>

        <section className="conv-brief__part" aria-labelledby={`brief-who-${slug}`}>
          <h2 id={`brief-who-${slug}`} className="conv-brief__title">
            Who it is
          </h2>
          <p className="conv-brief__persona" data-user-content>
            {persona || "No persona yet: it answers as Ghost would."}
          </p>
          <p className="conv-brief__meta">Speaks as {voiceTitle}</p>
          <Button size="sm" variant="secondary" leadingIcon={<SlidersHorizontal />} onClick={onPersona}>
            Voice and persona
          </Button>
        </section>

        <section className="conv-brief__part" aria-labelledby={`brief-mem-${slug}`}>
          <h2 id={`brief-mem-${slug}`} className="conv-brief__title">
            What it remembers
          </h2>
          <p className="conv-brief__meta">
            {detail.turn_count ? `${count(detail.turn_count)} turns` : "No turns yet"}
            {notes ? ` · ${notes} pinned ${notes === 1 ? "note" : "notes"}` : ""}. Its memory is its own; no other agent reads it.
          </p>
          <Button size="sm" variant="secondary" leadingIcon={<Brain />} onClick={onMemory}>
            Memory
          </Button>
        </section>

        <section className="conv-brief__part" aria-labelledby={`brief-reach-${slug}`}>
          <h2 id={`brief-reach-${slug}`} className="conv-brief__title">
            What it may reach
          </h2>
          <p className="conv-brief__meta">
            Every connected app ({reachApps} now) and Ghost’s own tools, under your permissions. Nothing narrows them per agent.
          </p>
          <span className="conv-brief__links">
            <Link to="/settings/apps">Apps</Link>
            <Link to="/settings/permissions">
              <ShieldCheck aria-hidden /> Permissions
            </Link>
          </span>
        </section>
      </div>
    </section>
  );
}
