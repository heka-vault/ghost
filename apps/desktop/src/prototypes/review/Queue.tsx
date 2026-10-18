/**
 * Owner: review. The ranked queue: a saved filter, one quiet line for what Ghost is on (or
 * where the last session stopped), then the issues waiting on the operator and what was
 * decided this session.
 *
 * The list is one listbox and one Tab stop (roving tabindex): arrows, Home and End move focus
 * and selection together, and J/K from anywhere on the page do the same (said under the list).
 * Rows are one height: a one-line title (the whole of it in the detail, and in a Tooltip
 * only when it's cut) and one quiet line under it. Rows are page-local rather than ListRow because ListRow can't take
 * a tabIndex or top-align (asked of foundation in shared_requests).
 */
import { type KeyboardEvent, type ReactNode, useLayoutEffect, useRef } from "react";

import type { ReviewFilter, ReviewIssue } from "../../design/mock/fixtures/review";
import { Card, cx, type GhostPhase, Kbd, Select, Skeleton, StatusDot, statusLabel } from "../../ui";
import { keyOf, sameRef, spoken, spokenRef, waited } from "./model";
import { Say, Truncated } from "./parts";

export interface QueueProps {
  waiting: ReviewIssue[];
  decided: ReviewIssue[];
  filters: ReviewFilter[];
  filter: ReviewFilter;
  counts: Record<string, number>;
  onFilter: (id: string) => void;
  selected: string | null;
  onSelect: (key: string) => void;
  discussing: { repo: string; number: number; phase: GhostPhase } | null;
  /** One quiet line above the list: what Ghost is on, or "continue where you left off". */
  context?: ReactNode;
  /** The filter matches nothing: the page draws one empty state across both columns. */
  empty?: boolean;
  /**
   * Height of the Page dock while a change waits. The list is bounded by the room above it
   * (plus the dock's fade, which the list may run under); the queue itself stays sticky.
   */
  dockHeight: number;
  /** Speak phrases only when Ghost can hear them. */
  canTalk: boolean;
  /** In a session phrases go as is; otherwise with the wake phrase in front. */
  inSession: boolean;
}

/** The edge fade's height (--space-8); reveal() keeps the selected row clear of it. */
const FADE = 32;
/** How much of the next row shows under the fade when the list is cut, so it reads as more. */
const PEEK = 24;
/** A row's height (title line plus meta line plus padding). */
const ROW = 52;
/** At the top of the page the list shows at least this many rows, even if its tail runs under the dock. */
const MIN_ROWS = 5;

const rowId = (k: string) => `rev-opt-${k}`;

/** A custom property in px, read off an element (--page-sticky-top on #main, --space-6 on :root). */
const px = (el: Element, name: string, fallback: number) => parseFloat(getComputedStyle(el).getPropertyValue(name)) || fallback;

export function Queue({ waiting, decided, filters, filter, counts, onFilter, selected, onSelect, discussing, context, empty, dockHeight, canTalk, inSession }: QueueProps) {
  const listRef = useRef<HTMLDivElement>(null);
  const scrollRef = useRef<HTMLDivElement>(null);
  const footRef = useRef<HTMLDivElement>(null);
  const rootRef = useRef<HTMLDivElement>(null);
  const order = [...waiting, ...decided].map(keyOf);

  /**
   * Keep the selected row in view: inside the list's own scroller first (scrollTop, not
   * scrollIntoView, so Chrome's Tab start stays put), then clear of the dock. At the top of
   * the page the list's tail may sit under the dock; a selection there scrolls the panel,
   * which slides the sticky queue up to its line and gives the list the room above the dock.
   * The first row of a group brings its label.
   */
  const reveal = () => {
    const box = scrollRef.current;
    const row = selected ? document.getElementById(rowId(selected)) : null;
    if (!box || !row) return;
    const head = row.dataset.first && row.parentElement ? row.parentElement : row;
    const b = box.getBoundingClientRect();
    const top = head.getBoundingClientRect().top;
    const bottom = row.getBoundingClientRect().bottom;
    // Clear of the edge fade, so the selected row is never half-faded.
    const pad = FADE + 4;
    if (top < b.top + pad) box.scrollTop -= b.top + pad - top;
    else if (bottom > b.bottom - pad) box.scrollTop += bottom - (b.bottom - pad);
    const main = document.getElementById("main");
    if (!main || !dockHeight) return;
    const fade = px(document.documentElement, "--space-6", 24);
    const limit = main.getBoundingClientRect().bottom - dockHeight + fade / 2;
    const after = row.getBoundingClientRect().bottom;
    if (after > limit) main.scrollBy({ top: after - limit, behavior: window.matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
  };
  const revealRef = useRef(reveal);
  revealRef.current = reveal;

  /** Fade the list's inner top or bottom edge only while there is more to scroll that way. */
  const edges = () => {
    const el = scrollRef.current;
    if (!el) return;
    const above = el.scrollTop > 1;
    const below = el.scrollTop + el.clientHeight < el.scrollHeight - 1;
    if (above) el.dataset.fadeTop = "";
    else delete el.dataset.fadeTop;
    if (below) el.dataset.fadeBottom = "";
    else delete el.dataset.fadeBottom;
  };

  // The queue is always sticky, so the ranked list is on screen wherever the operator reads
  // the detail, and J/K never move an invisible selection. Its list is bounded by the room
  // between where it sits now (lower than its sticky line before the panel scrolls; on the
  // line after) and the dock, plus the dock's fade: the list may run under the fade, which
  // dims its last row. At the top of a short panel it shows at least MIN_ROWS rows (never
  // more than fit once stuck), with its tail under the dock until the panel scrolls a little.
  // The first fit (and any change of the dock) reveals the selection once the bound is known.
  useLayoutEffect(() => {
    const main = document.getElementById("main");
    const el = scrollRef.current;
    const root = rootRef.current;
    if (!main || !el || !root) return;
    let raf = 0;
    let first = true;
    const fit = () => {
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        const gap = px(el, "--rev-gap", 16);
        // The card's own border sits outside the scroller; the phrase and the keys sit under the card.
        const card = el.parentElement;
        const frame = card ? card.offsetHeight - card.clientHeight : 2;
        const foot = footRef.current;
        const rowGap = foot?.parentElement ? parseFloat(getComputedStyle(foot.parentElement).rowGap) || 0 : 0;
        const say = foot ? foot.offsetHeight + rowGap + (parseFloat(getComputedStyle(foot).marginTop) || 0) : 0;
        const chrome = gap + frame + say;
        const fade = dockHeight ? px(document.documentElement, "--space-6", 24) : 0;
        const mainTop = main.getBoundingClientRect().top;
        const elTop = el.getBoundingClientRect().top;
        // Where the list sits once the queue is on its sticky line: the line, plus the
        // filter and the context line above the list.
        const stuckTop = (parseFloat(getComputedStyle(root).top) || 0) + (elTop - root.getBoundingClientRect().top);
        const roomStuck = main.clientHeight - stuckTop - dockHeight + fade - chrome;
        const roomNow = main.clientHeight - (elTop - mainTop) - dockHeight + fade - chrome;
        const room = Math.max(roomNow, Math.min(MIN_ROWS * ROW, roomStuck));
        // When the list doesn't fit, end it on a row's bottom plus a peek of the next row,
        // which the fade then dims: never a stray half row at full strength.
        let max = room;
        if (el.scrollHeight > room) {
          const origin = elTop - el.scrollTop;
          let best = 0;
          for (const r of el.querySelectorAll<HTMLElement>(".rev-row")) {
            const b = r.getBoundingClientRect().bottom - origin;
            if (b + PEEK <= room) best = b;
          }
          if (best > 0) max = best + PEEK;
        }
        el.style.maxHeight = `${max}px`;
        if (first) revealRef.current();
        first = false;
        edges();
      });
    };
    fit();
    main.addEventListener("scroll", fit, { passive: true });
    el.addEventListener("scroll", edges, { passive: true });
    const ro = new ResizeObserver(fit);
    ro.observe(main);
    ro.observe(el);
    return () => {
      cancelAnimationFrame(raf);
      main.removeEventListener("scroll", fit);
      el.removeEventListener("scroll", edges);
      ro.disconnect();
    };
  }, [dockHeight, !!context, !!empty, canTalk, inSession]);

  // A new selection: reveal it, and keep focus on it if focus is in the list (J/K, arrows).
  useLayoutEffect(() => {
    reveal();
    const row = selected ? document.getElementById(rowId(selected)) : null;
    const list = listRef.current;
    if (row && list && list.contains(document.activeElement) && document.activeElement !== row) row.focus({ preventScroll: true });
  }, [selected]);

  const onKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (e.key !== "ArrowDown" && e.key !== "ArrowUp" && e.key !== "Home" && e.key !== "End") return;
    const at = selected ? order.indexOf(selected) : -1;
    const next = e.key === "Home" ? 0 : e.key === "End" ? order.length - 1 : Math.max(0, Math.min(order.length - 1, at + (e.key === "ArrowDown" ? 1 : -1)));
    e.preventDefault();
    if (order[next] && order[next] !== selected) onSelect(order[next]);
  };

  const row = (i: ReviewIssue, n: number | null, setSize: number, first: boolean) => {
    const k = keyOf(i);
    const on = selected === k;
    const now = discussing && sameRef(discussing, i);
    const done = n == null ? i.decisions.find((d) => d.thisSession) : undefined;
    return (
      <div
        key={k}
        id={rowId(k)}
        role="option"
        aria-selected={on}
        aria-posinset={n ?? undefined}
        aria-setsize={n != null ? setSize : undefined}
        tabIndex={on ? 0 : -1}
        data-first={first || undefined}
        className={cx("rev-row", on && "is-selected")}
        onClick={(e) => {
          e.currentTarget.focus({ preventScroll: true });
          onSelect(k);
        }}
      >
        <span className="rev-row__top">
          <Truncated text={i.title} className="rev-row__title" />
          <span className="rev-row__time t-num">
            {n != null ? <>{waited(i.waitingSince)}<span className="sr-only"> waiting on you</span></> : done ? statusLabel(done.status) : null}
          </span>
        </span>
        <span className="rev-row__meta">
          <span className="rev-row__ref t-num">{spokenRef(i)}</span>
          {now && (
            <span className="rev-row__now">
              <StatusDot phase={discussing.phase} size="sm" still />
              On now
            </span>
          )}
          <span className="rev-row__sep" aria-hidden>·</span>
          <span className="rev-row__why">{done ? done.short ?? done.text : i.whyShort}</span>
        </span>
      </div>
    );
  };

  return (
    <div ref={rootRef} className="rev-queue">
      <div className="rev-queue__filter">
        <Select
          aria-label="Saved filter"
          aria-describedby={canTalk ? "rev-filter-say" : undefined}
          size="sm"
          value={filter.id}
          onChange={(e) => onFilter(e.target.value)}
          options={filters.map((f) => ({ value: f.id, label: `${f.label} · ${counts[f.id] ?? 0}` }))}
        />
      </div>

      {context}

      {!empty && (
        <>
          <Card padding="none" className="rev-queue__card">
            <div ref={scrollRef} className="rev-queue__scroll">
              <div ref={listRef} role="listbox" aria-label="Issues waiting on you, ranked" className="rev-list" onKeyDown={onKey}>
                {waiting.length > 0 && (
                  <div role="group" aria-label={`Waiting on you, ${waiting.length}`} className="rev-list__group">
                    {waiting.map((i, n) => row(i, n + 1, waiting.length, n === 0))}
                  </div>
                )}
                {decided.length > 0 && (
                  <div role="group" aria-labelledby="rev-decided-label" className="rev-list__group">
                    <p id="rev-decided-label" className="t-overline rev-list__label">Decided this session</p>
                    {decided.map((i, n) => row(i, null, 0, n === 0))}
                  </div>
                )}
              </div>
            </div>
          </Card>
          {/* How to move through it: the phrase to say, and the keys. Under the card, out of the list. */}
          <div ref={footRef} className="rev-queue__foot">
            {canTalk && <Say id="rev-filter-say" className="rev-queue__say rev-say--small" label="Or say" phrases={[spoken(filter.phrase, inSession)]} />}
            <p className="rev-queue__keys t-small t-3">
              <Kbd>J</Kbd> <Kbd>K</Kbd> to move
            </p>
          </div>
        </>
      )}
    </div>
  );
}

/** Shaped like the real thing: the filter, the one context line, and rows with a title and a meta line. */
export function QueueSkeleton() {
  return (
    <div className="rev-queue" aria-hidden>
      <div className="rev-queue__filter">
        <Skeleton width="100%" height={28} radius="var(--radius-md)" />
      </div>
      <div className="rev-skel-context">
        <Skeleton width={20} height={20} radius="var(--radius-sm)" />
        <Skeleton width="58%" height={12} />
      </div>
      <Card padding="none" className="rev-queue__card">
        {Array.from({ length: 6 }, (_, i) => (
          <div key={i} className="rev-skel-row">
            <span className="rev-skel-row__top">
              <Skeleton width={i % 2 ? "64%" : "82%"} height={12} />
              <Skeleton width={28} height={10} />
            </span>
            <Skeleton width={i % 2 ? "88%" : "70%"} height={10} />
          </div>
        ))}
      </Card>
    </div>
  );
}
