/**
 * The timeline: the spine of Today (docs/design-system.md, The timeline). One line down the
 * page from Now to midnight, each moment a mark on it in its kind's shape and colour, the time
 * in a gutter where it changes, the empty hours folded into one quiet gap. A run shows how long
 * it took and how much it did; three or more changes by one agent fold into one row, opened to
 * each.
 *
 * Moving through it: one Tab stop for the whole list (a roving focus), ↑/↓ or j/k between
 * moments, Home/End, Enter or Space to open one; the filter narrows by kind and by agent; the
 * day's map on the right jumps to an hour and shows where the day was busy.
 */
import type { ConversationView } from "@ghost/client-ts";
import { ChevronRight, ExternalLink } from "lucide-react";
import { type KeyboardEvent, type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { nameOf } from "../../app/today";
import { LineGlyph, SegmentedControl, Select, cx, scrollIntoPanel } from "../../ui";
import { type Moment, type MomentFilter, byHour, density, filterMoments, hourLabel, took } from "./model";

const FILTERS: { value: MomentFilter; label: string }[] = [
  { value: "all", label: "All" },
  { value: "changes", label: "Changes" },
  { value: "runs", label: "Runs" },
  { value: "watches", label: "Found" },
  { value: "questions", label: "Questions" },
];

const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

export function Timeline({ moments, agents, isToday, head }: { moments: Moment[]; agents: ConversationView[]; isToday: boolean; head: ReactNode }) {
  const [filter, setFilter] = useState<MomentFilter>("all");
  const [agent, setAgent] = useState<string>("");
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [focus, setFocus] = useState(0);
  const list = useRef<HTMLOListElement>(null);
  const shown = useMemo(() => filterMoments(moments, filter, agent || null), [moments, filter, agent]);
  const blocks = useMemo(() => byHour(shown), [shown]);
  const flat = useMemo(() => blocks.flatMap((b) => b.moments), [blocks]);
  const map = useMemo(() => density(shown), [shown]);
  const agentsHere = useMemo(() => {
    const slugs = new Set(moments.map((m) => m.agent).filter((a): a is string => !!a));
    return [...slugs].sort((a, b) => nameOf(agents, a).localeCompare(nameOf(agents, b)));
  }, [moments, agents]);
  const [visibleHour, setVisibleHour] = useState<number | null>(null);

  useEffect(() => setFocus((f) => Math.min(f, Math.max(0, flat.length - 1))), [flat.length]);

  // Which hour is at the top of the panel: the map marks it.
  useEffect(() => {
    const root = document.getElementById("main");
    const el = list.current;
    if (!root || !el) return;
    const io = new IntersectionObserver(
      (entries) => {
        const top = entries.filter((e) => e.isIntersecting).sort((a, b) => a.boundingClientRect.top - b.boundingClientRect.top)[0];
        if (top) setVisibleHour(Number((top.target as HTMLElement).dataset.hour));
      },
      { root, rootMargin: "-20% 0px -60% 0px" },
    );
    el.querySelectorAll("[data-hour]").forEach((n) => io.observe(n));
    return () => io.disconnect();
  }, [blocks]);

  const toggle = (id: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  const moveTo = (i: number) => {
    const n = Math.max(0, Math.min(flat.length - 1, i));
    setFocus(n);
    const el = list.current?.querySelector<HTMLElement>(`[data-index="${n}"]`);
    el?.focus({ preventScroll: true });
    if (el) scrollIntoPanel(el, { block: "nearest" });
  };
  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (e.target instanceof HTMLElement && e.target.closest("a, button:not(.tl-row__hit)")) return;
    const k = e.key;
    if (k === "ArrowDown" || k === "j") moveTo(focus + 1);
    else if (k === "ArrowUp" || k === "k") moveTo(focus - 1);
    else if (k === "Home") moveTo(0);
    else if (k === "End") moveTo(flat.length - 1);
    else if (k === "Enter" || k === " ") {
      const m = flat[focus];
      if (m && (m.children || m.detail)) toggle(m.id);
    } else return;
    e.preventDefault();
  };
  const jump = (hour: number) => {
    const el = list.current?.querySelector<HTMLElement>(`[data-hour="${hour}"]`);
    if (el) scrollIntoPanel(el, { block: "start" });
  };

  let index = -1;
  const nowLabel = new Date().toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

  return (
    <div className="tl-wrap">
      <div className="tl-main">
        {isToday && (
          <div className="tl-now">
            <span className="tl-gutter t-num">{nowLabel}</span>
            <span className="tl-mark tl-mark--now" aria-hidden />
            <div className="tl-now__body">
              <h2 className="tl-now__title">Now</h2>
              {head}
            </div>
          </div>
        )}

        <div className="tl-tools">
          <span className="tl-gutter" />
          <span className="tl-tools__line" aria-hidden />
          <div className="tl-tools__row">
            <h2 className="tl-tools__title">{isToday ? "Earlier today" : "That day"}</h2>
            <SegmentedControl size="sm" aria-label="Show" options={FILTERS} value={filter} onChange={(v) => setFilter(v as MomentFilter)} />
            {agentsHere.length > 1 && (
              <Select size="sm" aria-label="Agent" value={agent} onChange={(e) => setAgent(e.target.value)} options={[{ value: "", label: "Every agent" }, ...agentsHere.map((a) => ({ value: a, label: nameOf(agents, a) }))]} />
            )}
          </div>
        </div>

        {flat.length === 0 ? (
          <div className="tl-empty">
            <span className="tl-gutter" />
            <span className="tl-tools__line" aria-hidden />
            <p>
              {moments.length === 0
                ? isToday
                  ? "Nothing has happened yet today. When Ghost or an agent changes something, a run finishes, a watch finds something or a question is asked, it lands here, newest first."
                  : "Nothing happened that day."
                : "Nothing of that kind. Show all to see the rest."}
            </p>
          </div>
        ) : (
          <ol className="tl" ref={list} aria-label={isToday ? "What happened today" : "What happened that day"} onKeyDown={onKey}>
            {blocks.map((b) => (
              <li key={b.hour} className="tl-hour" data-hour={b.hour}>
                {b.quietBefore > 0 && (
                  <div className="tl-gap" aria-hidden>
                    <span className="tl-gutter" />
                    <span className="tl-gap__line" />
                    <span className="tl-gap__text">
                      quiet · {b.quietBefore} {b.quietBefore === 1 ? "hour" : "hours"}
                    </span>
                  </div>
                )}
                <h3 className="tl-hour__label">
                  <span className="tl-gutter t-num">{hourLabel(b.hour)}</span>
                </h3>
                <ol className="tl-hour__list" role="list">
                  {b.moments.map((m) => {
                    index += 1;
                    return <Row key={m.id} m={m} index={index} focused={index === focus} open={open.has(m.id)} onToggle={() => toggle(m.id)} onFocus={setFocus} agents={agents} />;
                  })}
                </ol>
              </li>
            ))}
            <li className="tl-end" aria-hidden>
              <span className="tl-gutter t-num">12 AM</span>
              <span className="tl-mark tl-mark--end" />
              <span className="tl-end__text">{isToday ? "The day began" : "Start of the day"}</span>
            </li>
          </ol>
        )}
      </div>

      {flat.length > 0 && (
      <nav className="tl-map" aria-label="Hours of the day">
        <p className="tl-map__title">The day</p>
        <ol role="list">
          {map
            .slice()
            .reverse()
            .map((h) => {
              const max = Math.max(1, ...map.map((x) => x.count));
              const future = isToday && h.hour > new Date().getHours();
              return (
                <li key={h.hour}>
                  <button
                    type="button"
                    className={cx("tl-map__hour", visibleHour === h.hour && "is-here", future && "is-future", h.failed > 0 && "has-failed")}
                    disabled={!h.count}
                    onClick={() => jump(h.hour)}
                    aria-label={`${hourLabel(h.hour)}: ${h.count} ${h.count === 1 ? "moment" : "moments"}`}
                  >
                    <span className="tl-map__label t-num">{h.hour % 3 === 0 ? hourLabel(h.hour) : ""}</span>
                    <span className="tl-map__bar" style={{ inlineSize: `${(h.count / max) * 100}%` }} />
                  </button>
                </li>
              );
            })}
        </ol>
      </nav>
      )}
    </div>
  );
}

function Row({ m, index, focused, open, onToggle, onFocus, agents }: { m: Moment; index: number; focused: boolean; open: boolean; onToggle: () => void; onFocus: (i: number) => void; agents: ConversationView[] }) {
  const who = m.agent ? nameOf(agents, m.agent) : null;
  const opens = !!(m.children?.length || (m.detail && m.detail.length > 0));
  const sentence = rowSentence(m, who);
  return (
    <li className={cx("tl-row", `is-${m.kind}`, `tone-${m.tone}`, open && "is-open")}>
      <span className="tl-gutter t-num">{time(m.at)}</span>
      <span className={cx("tl-mark", `tl-mark--${m.kind}`)} aria-hidden />
      <div className="tl-row__body">
        <button
          type="button"
          className="tl-row__hit"
          data-index={index}
          tabIndex={focused ? 0 : -1}
          aria-expanded={opens ? open : undefined}
          onFocus={() => onFocus(index)}
          onClick={opens ? onToggle : undefined}
        >
          {who && <LineGlyph slug={m.agent!} name={who} size="sm" />}
          <span className="tl-row__text">
            {who && <strong className="tl-row__who">{who}</strong>}
            <span className="tl-row__verb">{sentence.verb}</span> <span data-user-content>{sentence.what}</span>
          </span>
          <span className="tl-row__meta">{sentence.meta}</span>
          {opens && <ChevronRight className="tl-row__chev" aria-hidden />}
        </button>
        {m.detail && !open && m.kind === "run" && (
          <p className="tl-row__peek" data-user-content>
            {m.detail}
          </p>
        )}
        {open && (
          <div className="tl-row__open">
            {m.detail && (
              <p className="tl-row__detail" data-user-content>
                {m.detail}
              </p>
            )}
            {m.children && (
              <ol className="tl-row__children" role="list">
                {m.children.map((c) => (
                  <li key={c.id} className={cx("tl-child", `tone-${c.tone}`)}>
                    <span className="tl-child__time t-num">{time(c.at)}</span>
                    <span className="tl-child__mark" aria-hidden />
                    <span data-user-content>{c.title}</span>
                  </li>
                ))}
              </ol>
            )}
            {(m.mono || m.link) && (
              <p className="tl-row__mono">
                {m.mono && <span className="t-mono">{m.mono}</span>}
                {m.link && (
                  <a href={m.link} target="_blank" rel="noreferrer" className="tl-row__link">
                    Open <ExternalLink aria-hidden />
                  </a>
                )}
              </p>
            )}
          </div>
        )}
      </div>
    </li>
  );
}

/** A moment as a sentence: who, what they did, and a quiet meta part. */
export function rowSentence(m: Moment, who: string | null): { verb: string; what: string; meta: string } {
  switch (m.kind) {
    case "change":
      if (m.children) return { verb: m.tone === "failed" ? "tried" : "made", what: m.title, meta: m.detail ?? "" };
      return { verb: m.tone === "failed" ? "couldn’t:" : m.tone === "quiet" ? "left alone:" : "", what: m.title, meta: m.tone === "failed" ? "failed" : m.detail ?? "" };
    case "run": {
      const length = m.ends ? took(m.at, m.ends) : m.tone === "running" ? "running" : "";
      const calls = m.calls && m.calls.tool + m.calls.model > 0 ? `${m.calls.tool} tool ${m.calls.tool === 1 ? "call" : "calls"}` : "no model needed";
      const verb = m.tone === "failed" ? "couldn’t finish" : m.tone === "running" ? "is running" : "ran";
      return { verb, what: m.title, meta: [m.mono, length, calls].filter(Boolean).join(" · ") };
    }
    case "watch":
      return { verb: "", what: m.title, meta: m.mono ?? "" };
    case "question":
      return { verb: who ? "asked:" : `${m.mono ?? "Someone"} asked:`, what: m.title, meta: m.tone === "running" ? "waiting for you" : "" };
    case "answer":
      return { verb: "", what: m.title, meta: m.mono ?? "" };
    default:
      return { verb: who ? "said" : "", what: m.title, meta: m.mono ?? "" };
  }
}
