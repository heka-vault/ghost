/**
 * Owner: audit. Every change Ghost made or asked to make: what, on what, at which level,
 * and how it went. Grouped by day, newest first; each row opens onto the gate's own event
 * trail. Search and filters live in the page header's actions, so they ride along in the
 * compact bar while 200 rows scroll; each day's title sticks under the bar. Filters and
 * search run over what is loaded, and every count says so; "Show older" loads more.
 *
 * URL states (query before the hash): status=<all|attention|executed|pending|…>,
 * ability=<source name>, q=<text>, open=<first|id>, fail=audit (mock only: every request fails),
 * fail=audit-more (mock only: the first page loads, "Show older" fails).
 */
import type { AuditEntry } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ScrollText, Search, SearchX, X } from "lucide-react";
import { type KeyboardEvent, type RefObject, useContext, useEffect, useId, useLayoutEffect, useMemo, useRef, useState } from "react";

import { hrefFor } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import { Button, ButtonSizeContext, Card, EmptyState, IconButton, Input, Kbd, LiveStatus, Notice, Page, Select, Skeleton, errorText, formatTime, useLive } from "../../ui";
import { AuditRow, type LiveChange } from "./AuditRow";
import { ATTENTION, STATUS_ORDER, groupByDay, haystack, initialParams, matches, outcomeLabel, outcomeOf, sourceLabel } from "./model";
import { MAX_LOADED, PAGE, useAuditLog } from "./useAuditLog";
import "./audit.css";

export default function AuditScreen(_: RouteProps) {
  const { client, lastEvent, pending, turns, conversationsVersion } = useGhost();
  const { online } = useLive();
  const design = useDesign();
  const init = useMemo(initialParams, []);
  // What can change the log: a change starts or stops waiting, a write settles, a line is
  // edited, or a turn lands. The turn count stands in for outcome events that React batches
  // away behind the status event that follows them; phase changes alone never refetch.
  const outcomeKey = lastEvent?.type === "outcome" ? lastEvent.outcome.proposal_id : "";
  const signal = `${pending?.id ?? "-"}|${outcomeKey}|${conversationsVersion}|${turns.length}`;
  const log = useAuditLog(client, signal, { fail: !!design && init.fail, failMore: !!design && init.failMore });

  // The change waiting right now, answered from its own row: Confirm calls client.confirm()
  // and nothing else (the click is confirmation evidence, ADR 0013), Cancel calls
  // client.cancel(). The daemon's pending event settles the row; an error shows under it.
  const [answering, setAnswering] = useState<"confirm" | "cancel" | null>(null);
  const [answerError, setAnswerError] = useState<unknown>(null);
  useEffect(() => {
    setAnswering(null);
    setAnswerError(null);
  }, [pending?.id]);
  const answer = (action: "confirm" | "cancel") => {
    setAnswering(action);
    setAnswerError(null);
    (action === "confirm" ? client.confirm() : client.cancel()).catch((e: unknown) => {
      setAnswerError(e);
      setAnswering(null);
    });
  };
  const live: LiveChange | null = pending
    ? {
        summary: pending.summary,
        rationale: pending.rationale,
        operation: `${pending.ability}.${pending.operation}`,
        expiresAt: pending.expires_at,
        busy: answering ?? false,
        disabled: !online,
        error: answerError,
        onConfirm: () => answer("confirm"),
        onCancel: () => answer("cancel"),
      }
    : null;

  const [status, setStatus] = useState(init.status);
  const [ability, setAbility] = useState(init.ability);
  const [query, setQuery] = useState(init.q);
  const [open, setOpen] = useState<Set<string>>(() => new Set());
  const [cursor, setCursor] = useState<string | null>(null);
  const root = useRef<HTMLDivElement>(null);
  const search = useRef<HTMLInputElement>(null);
  const statusSelect = useRef<HTMLSelectElement>(null);
  const list = useRef<HTMLDivElement>(null);
  const rows = useRef(new Map<string, HTMLButtonElement>());

  // "/" focuses the search from anywhere on the page.
  useEffect(() => {
    const onKey = (e: globalThis.KeyboardEvent) => {
      if (e.key !== "/" || e.ctrlKey || e.metaKey || e.altKey) return;
      const t = e.target as HTMLElement | null;
      if (t && (t.closest("input, textarea, select, [contenteditable='true']") || t.isContentEditable)) return;
      if (!search.current) return;
      e.preventDefault();
      search.current.focus();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, []);

  useStickyDays(root);

  const byId = useMemo(() => new Map(log.items.map((e) => [e.id, e])), [log.items]);
  // The daemon's word on what is waiting now. With it, a stored "pending" that isn't this id
  // lapsed; without it (not connected), only a row inside the confirmation window is Waiting.
  const liveId = online ? (pending?.id ?? "") : null;
  const ctx = useMemo(() => ({ liveId, now: log.updatedAt ?? Date.now() }), [liveId, log.updatedAt]);
  const hay = useMemo(() => new Map(log.items.map((e) => [e.id, haystack(e, ctx)])), [log.items, ctx]);

  const counts = useMemo(() => {
    const c = new Map<string, number>();
    let attention = 0;
    for (const e of log.items) {
      if (ability !== "all" && e.ability !== ability) continue;
      const o = outcomeOf(e, ctx);
      c.set(o, (c.get(o) ?? 0) + 1);
      if (ATTENTION.has(o)) attention++;
    }
    return { by: c, attention };
  }, [log.items, ability, ctx]);

  const abilities = useMemo(() => [...new Set(log.items.map((e) => e.ability))].sort(), [log.items]);

  const filtered = useMemo(() => {
    const q = query.trim();
    return log.items.filter((e) => {
      if (ability !== "all" && e.ability !== ability) return false;
      const o = outcomeOf(e, ctx);
      if (status === "attention" ? !ATTENTION.has(o) : status !== "all" && o !== status) return false;
      return !q || matches(hay.get(e.id) ?? "", q);
    });
  }, [log.items, ability, status, query, hay, ctx]);

  const groups = useMemo(() => groupByDay(filtered), [filtered]);

  // ?open=first|<id> once the first page is in: "first" is the first row the filters show.
  const opened = useRef(false);
  useEffect(() => {
    if (opened.current || !init.open || !log.items.length) return;
    opened.current = true;
    const id = init.open === "first" ? filtered[0]?.id : init.open;
    if (id) setOpen(new Set([id]));
  }, [init.open, log.items, filtered]);

  const filtering = status !== "all" || ability !== "all" || query.trim() !== "";
  const more = log.items.length < log.total;
  const left = Math.min(PAGE, log.total - log.items.length);

  // With filters on and nothing matching, "Search older changes" keeps loading pages until a
  // match appears, the log ends, a page fails, or the API's limit is reached, with the button
  // loading the whole way. A filter change while it runs stops it.
  const [searching, setSearching] = useState(false);
  useEffect(() => {
    if (!searching) return;
    if (filtered.length > 0 || !more || log.moreError != null || !online || log.items.length >= MAX_LOADED) {
      setSearching(false);
      return;
    }
    if (!log.loadingMore) log.loadMore();
  }, [searching, filtered.length, more, online, log]);
  useEffect(() => setSearching(false), [filterKeyOf(status, ability, query)]);

  const reset = () => {
    setStatus("all");
    setAbility("all");
    setQuery("");
  };
  // From a button that is about to unmount: keep the keyboard user's place.
  const clearFilters = () => {
    reset();
    requestAnimationFrame(() => search.current?.focus());
  };
  const showAttention = () => {
    setStatus("attention");
    requestAnimationFrame(() => statusSelect.current?.focus());
  };

  const toggle = (id: string) =>
    setOpen((s) => {
      const n = new Set(s);
      if (n.has(id)) n.delete(id);
      else n.add(id);
      return n;
    });

  // The list is one Tab stop (roving tabindex): the last row you were on, or the first.
  const activeId = cursor && filtered.some((e) => e.id === cursor) ? cursor : (filtered[0]?.id ?? null);
  const onListKey = (e: KeyboardEvent<HTMLDivElement>) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    const t = e.target as HTMLElement;
    if (!t.matches("[data-audit-row]")) return;
    const all = [...(list.current?.querySelectorAll<HTMLButtonElement>("[data-audit-row]") ?? [])];
    const i = all.indexOf(t as HTMLButtonElement);
    const next = e.key === "Home" ? 0 : e.key === "End" ? all.length - 1 : i + (e.key === "ArrowDown" ? 1 : -1);
    if (next < 0 || next >= all.length) return;
    e.preventDefault();
    all[next].focus({ preventScroll: true });
    revealRow(all[next], "nearest");
  };

  // Filter results, announced once the operator stops typing; never the "Updated" time.
  const [matchText, setMatchText] = useState("");
  const firstFilter = useRef(true);
  const filterKey = filterKeyOf(status, ability, query);
  const matchCount = filtered.length;
  useEffect(() => {
    if (firstFilter.current) {
      firstFilter.current = false;
      return;
    }
    const t = setTimeout(() => {
      if (status === "all" && ability === "all" && !query.trim()) setMatchText("Showing every loaded change");
      else setMatchText(matchCount === 0 ? "No loaded changes match" : `${matchCount} ${matchCount === 1 ? "change matches" : "changes match"}`);
    }, 450);
    return () => clearTimeout(t);
    // Only a filter change announces; a live refresh that changes the count stays quiet.
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [filterKey]);

  // One polite line for live arrivals.
  const freshText = useMemo(() => {
    const n = [...log.fresh].map((id) => byId.get(id)).filter(Boolean) as AuditEntry[];
    if (!n.length) return "";
    return n.length === 1 ? `New change: ${n[0].summary}` : `${n.length} new changes`;
  }, [log.fresh, byId]);

  const offlineReason = useId();
  const updated = log.updatedAt ? formatTime(new Date(log.updatedAt).toISOString()) : null;
  const q = query.trim();
  const noMatchTitle = `Nothing${more ? ` in the newest ${log.items.length}` : ""} matches ${q ? `“${q}”` : "these filters"}`;
  // The automatic search stops at the API's limit; past it, the regular "Show older" takes over.
  const atLimit = log.items.length >= MAX_LOADED;
  const noMatchDescription =
    q && (status !== "all" || ability !== "all")
      ? `The search only covers the status and source you picked.${more ? " Older changes may match too." : ""}`
      : more && atLimit
        ? `The newest ${log.items.length} were searched; older changes may match.`
        : more
          ? `Older changes aren’t loaded yet, and may match.${online ? "" : " They load once Ghost is back."}`
          : q
            ? "Try fewer words, an issue number, or an operation like close."
            : "Try another status or source.";
  const empty = !log.loading && !log.error && log.total === 0 && log.items.length === 0;

  const actions = log.loading ? (
    <ToolbarSkeleton />
  ) : log.items.length > 0 ? (
    <AuditToolbar
      search={search}
      statusRef={statusSelect}
      query={query}
      setQuery={setQuery}
      status={status}
      setStatus={setStatus}
      ability={ability}
      setAbility={setAbility}
      abilities={abilities}
      counts={counts}
      scope={more ? log.items.length : null}
    />
  ) : null;

  return (
    <Page width="wide" title="Audit log" description="Every change Ghost made or asked to make, and how it went." status={<LiveStatus />} actions={actions}>
      <div className="audit" ref={root}>
        {log.error != null && !log.loading && (log.items.length === 0 || online) && (
          <Notice
            tone="danger"
            title={log.items.length ? "Couldn’t refresh the log" : "Couldn’t load the audit log"}
            action={
              <Button size="sm" variant="secondary" onClick={log.retry}>
                Try again
              </Button>
            }
          >
            {errorText(log.error)}
            {log.items.length ? " What’s below is from the last time it loaded." : ""}
          </Notice>
        )}

        {log.loading && <AuditSkeleton />}

        {empty && (
          <Card padding="none">
            <EmptyState
              icon={<ScrollText />}
              title="No changes yet"
              description={"When Ghost changes something, or asks\u00a0to, it shows up here with every step it took."}
              actions={
                <Button variant="secondary" size="sm" href={hrefFor("/policy")}>
                  See what Ghost may do
                </Button>
              }
            />
          </Card>
        )}

        {!log.loading && log.items.length > 0 && (
          <>
            <div className="audit-summaryline t-small">
              {filtered.length > 0 && (
                <>
                  {filtering ? (
                    <span>
                      Showing <span className="t-num t-1 t-medium">{filtered.length}</span> of <span className="t-num">{log.items.length}</span>
                      {more ? " loaded" : ""} {log.items.length === 1 ? "change" : "changes"}
                    </span>
                  ) : more ? (
                    <span>
                      The newest <span className="t-num">{log.items.length}</span> of <span className="t-num">{log.total}</span> changes
                    </span>
                  ) : (
                    <span>
                      <span className="t-num">{log.total}</span> {log.total === 1 ? "change" : "changes"}
                    </span>
                  )}
                  {!filtering && counts.attention > 0 && (
                    <>
                      <span aria-hidden className="audit-row__sep">·</span>
                      <button type="button" className="audit-attention" onClick={showAttention}>
                        <span className="audit-attention__dot" aria-hidden />
                        <span className="t-num">{`${counts.attention} ${counts.attention === 1 ? "needs" : "need"} a look`}</span>
                      </button>
                    </>
                  )}
                  {filtering && (
                    <>
                      <span aria-hidden className="audit-row__sep">·</span>
                      <button type="button" className="audit-link" onClick={clearFilters}>
                        Clear filters
                      </button>
                    </>
                  )}
                </>
              )}
              {updated && (
                <span className="audit-updated t-3" title={online ? undefined : "Ghost isn’t answering. These rows are from before it went away."}>
                  {online ? "Updated" : "Last updated"} <span className="t-num">{updated}</span>
                </span>
              )}
            </div>
            <p className="sr-only" aria-live="polite">{matchText}</p>
            <p className="sr-only" aria-live="polite">{freshText}</p>

            {filtered.length === 0 ? (
              <Card padding="none">
                <EmptyState
                  size="compact"
                  icon={<SearchX />}
                  title={noMatchTitle}
                  description={noMatchDescription}
                  actions={
                    <>
                      <Button size="sm" variant="secondary" onClick={clearFilters}>
                        Clear filters
                      </Button>
                      {more && !log.moreError && (atLimit ? (
                        <Button size="sm" variant="ghost" loading={log.loadingMore} disabled={!online} onClick={log.loadMore}>
                          Show {left} older changes
                        </Button>
                      ) : (
                        <Button size="sm" variant="ghost" loading={searching || log.loadingMore} disabled={!online} onClick={() => setSearching(true)}>
                          Search older changes
                        </Button>
                      ))}
                    </>
                  }
                />
              </Card>
            ) : (
              <div className="audit-days" ref={list} onKeyDown={onListKey}>
                {groups.map((g) => (
                  <section key={g.key} className="audit-day" aria-labelledby={`audit-day-${g.key}`}>
                    <h2 id={`audit-day-${g.key}`} className="audit-day__title">
                      {g.label}
                      <span className="audit-day__count t-num">{g.items.length}</span>
                    </h2>
                    <Card list role="list" className="audit-card">
                      {g.items.map((e) => (
                        <AuditRow
                          key={e.id}
                          ref={(el) => {
                            if (el) rows.current.set(e.id, el);
                            else rows.current.delete(e.id);
                          }}
                          entry={e}
                          highlight={query}
                          open={open.has(e.id)}
                          onToggle={() => toggle(e.id)}
                          tabIndex={e.id === activeId ? 0 : -1}
                          onFocus={() => setCursor(e.id)}
                          fresh={log.fresh.has(e.id)}
                          live={pending?.id === e.id ? live : null}
                          outcome={outcomeOf(e, ctx)}
                        />
                      ))}
                    </Card>
                  </section>
                ))}
              </div>
            )}

            {(more && log.moreError != null) && (
              <Notice
                tone="danger"
                title="Couldn’t load older changes"
                action={
                  <Button size="sm" variant="secondary" loading={log.loadingMore} disabled={!online} onClick={log.loadMore}>
                    Try again
                  </Button>
                }
              >
                {errorText(log.moreError)}
              </Notice>
            )}

            {filtered.length > 0 && (
              <div className="audit-foot">
                {more ? (
                  <>
                    {!log.moreError && (
                      <Button variant="secondary" loading={log.loadingMore} disabled={!online} aria-describedby={online ? undefined : offlineReason} onClick={log.loadMore}>
                        Show {left} older changes
                      </Button>
                    )}
                    <span className="t-small t-3" id={online ? undefined : offlineReason}>
                      {online ? (
                        <>
                          <span className="t-num">{log.items.length}</span> of <span className="t-num">{log.total}</span> loaded
                        </>
                      ) : (
                        "Older changes load once Ghost is back."
                      )}
                    </span>
                  </>
                ) : (
                  <span className="t-small t-3">
                    That’s everything{log.items.length > 1 ? ` since ${new Date(log.items[log.items.length - 1].created_at).toLocaleDateString(undefined, { month: "short", day: "numeric" })}` : ""}.
                  </span>
                )}
              </div>
            )}
          </>
        )}
      </div>
    </Page>
  );
}

const filterKeyOf = (status: string, ability: string, query: string) => `${status}|${ability}|${query.trim()}`;

/* ------------------------------------------------------------------------------ toolbar */

interface ToolbarProps {
  search: RefObject<HTMLInputElement | null>;
  statusRef: RefObject<HTMLSelectElement | null>;
  query: string;
  setQuery: (q: string) => void;
  status: string;
  setStatus: (s: string) => void;
  ability: string;
  setAbility: (a: string) => void;
  abilities: string[];
  counts: { by: Map<string, number>; attention: number };
  /** How many rows the counts cover when not every row is loaded; null when they cover the whole log. */
  scope: number | null;
}

/**
 * Search and the two filters, in the page's actions: they move into the compact bar as the
 * page scrolls (at size sm there), so filtering stays in reach down a 200-row log.
 */
function AuditToolbar({ search, statusRef, query, setQuery, status, setStatus, ability, setAbility, abilities, counts, scope }: ToolbarProps) {
  const size = useContext(ButtonSizeContext) ?? "md";
  // The "/" hint shows only while the field is empty and unfocused: once focused, "/" types a slash.
  const [focused, setFocused] = useState(false);
  const abilitySelect = useRef<HTMLSelectElement>(null);
  useFitSelect(statusRef, `${status}|${size}|${counts.attention}|${[...counts.by].join()}`);
  useFitSelect(abilitySelect, `${ability}|${size}|${abilities.join()}`);
  const statuses = STATUS_ORDER.filter((s) => counts.by.has(s) || s === status);
  const counted = (
    <>
      <option value="attention">{`Needs a look · ${counts.attention}`}</option>
      {statuses.map((s) => (
        <option key={s} value={s}>{`${outcomeLabel(s)} · ${counts.by.get(s) ?? 0}`}</option>
      ))}
    </>
  );
  return (
    <div className="audit-toolbar" role="search" aria-label="Filter the audit log">
      <Input
        ref={search}
        size={size}
        className="audit-search"
        type="search"
        value={query}
        onChange={(e) => setQuery(e.target.value)}
        onFocus={() => setFocused(true)}
        onBlur={() => setFocused(false)}
        onKeyDown={(e) => {
          if (e.key === "Escape" && query) {
            e.preventDefault();
            setQuery("");
          }
        }}
        placeholder="Search #151, close…"
        aria-label="Search loaded changes"
        leading={<Search />}
        trailing={query ? <IconButton size="sm" variant="ghost" label="Clear search" icon={<X />} onClick={() => (setQuery(""), search.current?.focus())} /> : focused ? undefined : <Kbd>/</Kbd>}
      />
      <Select ref={statusRef} size={size} className="audit-filter" aria-label="Status" value={status} onChange={(e) => setStatus(e.target.value)}>
        <option value="all">All statuses</option>
        {/* The counts cover what is loaded; say so once, where they are read. */}
        {scope != null ? <optgroup label={`Counted in the newest ${scope}`}>{counted}</optgroup> : counted}
      </Select>
      {abilities.length > 1 && (
        <Select
          ref={abilitySelect}
          size={size}
          className="audit-filter"
          aria-label="Source"
          value={ability}
          onChange={(e) => setAbility(e.target.value)}
          options={[{ value: "all", label: "All sources" }, ...abilities.map((a) => ({ value: a, label: sourceLabel(a) }))]}
        />
      )}
    </div>
  );
}

/**
 * A native select is as wide as its widest option. This sizes it to the option it shows
 * ("All sources", "Failed · 3"), measured in its own font, so the filters read as compact
 * chips; the open list can still be wider. Re-measures when the label or the size changes,
 * and once the web font has loaded.
 */
function useFitSelect(ref: RefObject<HTMLSelectElement | null>, key: string) {
  useLayoutEffect(() => {
    const el = ref.current;
    if (!el) return;
    const fit = () => {
      const label = el.selectedOptions[0]?.text ?? "";
      const cs = getComputedStyle(el);
      const ctx = (fitCanvas ??= document.createElement("canvas")).getContext("2d");
      if (!ctx) return;
      ctx.font = `${cs.fontWeight} ${cs.fontSize} ${cs.fontFamily}`;
      ctx.fontKerning = "normal";
      el.style.width = `${Math.ceil(ctx.measureText(label).width) + 4}px`;
    };
    fit();
    let alive = true;
    void document.fonts?.ready.then(() => alive && fit());
    return () => {
      alive = false;
    };
  }, [ref, key]);
}
let fitCanvas: HTMLCanvasElement | null = null;

function ToolbarSkeleton() {
  const size = useContext(ButtonSizeContext) ?? "md";
  const h = `var(--control-${size})`;
  return (
    <div className="audit-toolbar" aria-hidden>
      <Skeleton height={h} radius="var(--radius-md)" className="audit-search" />
      <Skeleton height={h} radius="var(--radius-md)" className="audit-skel-filter" />
      <Skeleton height={h} radius="var(--radius-md)" className="audit-skel-filter audit-skel-filter--short" />
    </div>
  );
}

function AuditSkeleton() {
  return (
    <div className="audit-skeleton" aria-busy="true" aria-label="Loading the audit log">
      <div className="audit-summaryline">
        <Skeleton width="22%" height="var(--space-3)" />
      </div>
      <div className="audit-day">
        <div className="audit-day__title audit-day__title--skel">
          <Skeleton height="var(--space-3)" className="audit-skel-day" />
        </div>
        <Card list className="audit-card">
          {Array.from({ length: 9 }, (_, i) => (
            <div key={i} className="audit-skel-row">
              <span className="audit-skel-row__main">
                <span className="audit-skel-row__line">
                  <Skeleton width={`${[72, 58, 80, 64, 70, 52, 76, 61, 68][i]}%`} height="var(--space-3)" />
                </span>
                <span className="audit-skel-row__meta">
                  <Skeleton width={`${[34, 40, 30, 38, 28, 36, 32, 42, 30][i]}%`} height="var(--space-2_5)" />
                </span>
              </span>
              <Skeleton height="var(--count-size)" radius="var(--radius-full)" className="audit-skel-badge" />
              <Skeleton height="var(--space-3)" className="audit-skel-time" />
              <span />
            </div>
          ))}
        </Card>
      </div>
    </div>
  );
}

/* ------------------------------------------------------------------------------ scrolling */

/**
 * Day titles stick under the compact bar. The bar sits under any connection banner, so the
 * stick line is the shell's banner stack plus the bar; it is measured, because a banner
 * comes and goes (reconnecting). A stuck title gets a hairline, like the bar's.
 */
function useStickyDays(root: RefObject<HTMLDivElement | null>) {
  useEffect(() => {
    const el = root.current;
    const scroller = el?.closest<HTMLElement>(".shell__main");
    if (!el || !scroller) return;
    const top = scroller.querySelector<HTMLElement>(".shell__top");
    const setTop = () => el.style.setProperty("--audit-banners", `${top?.offsetHeight ?? 0}px`);
    setTop();
    const ro = top ? new ResizeObserver(setTop) : null;
    if (top) ro?.observe(top);
    let raf = 0;
    const check = () => {
      raf = 0;
      const titles = el.querySelectorAll<HTMLElement>(".audit-day__title");
      if (!titles.length) return;
      const line = parseFloat(getComputedStyle(titles[0]).top) || 0;
      const s = scroller.getBoundingClientRect().top + scroller.clientTop;
      for (const t of titles) {
        const top = t.getBoundingClientRect().top - s;
        const end = (t.parentElement?.getBoundingClientRect().bottom ?? 0) - s;
        t.classList.toggle("is-stuck", scroller.scrollTop > 0 && top <= line + 1 && end > top + t.offsetHeight);
      }
    };
    const onScroll = () => {
      if (!raf) raf = requestAnimationFrame(check);
    };
    scroller.addEventListener("scroll", onScroll, { passive: true });
    const mo = new MutationObserver(onScroll);
    mo.observe(el, { childList: true, subtree: true });
    check();
    return () => {
      scroller.removeEventListener("scroll", onScroll);
      ro?.disconnect();
      mo.disconnect();
      if (raf) cancelAnimationFrame(raf);
    };
  }, [root]);
}

/**
 * scrollIntoPanel for a row under a sticky day title: like it, never scrollIntoView (which
 * would move Chrome's Tab starting point), but it keeps the row clear of the compact bar and
 * its day's title. See shared_requests: scrollIntoPanel could honor a top inset itself.
 */
function revealRow(el: HTMLElement, block: "start" | "nearest") {
  const scroller = el.closest<HTMLElement>(".shell__main");
  if (!scroller) return;
  const title = el.closest(".audit-day")?.querySelector<HTMLElement>(".audit-day__title");
  // The stuck title's canvas band (--space-2) hides a little more below its hairline.
  const band = parseFloat(getComputedStyle(scroller).getPropertyValue("--space-2")) || 0;
  const inset = title ? (parseFloat(getComputedStyle(title).top) || 0) + title.offsetHeight + band : 0;
  const s = scroller.getBoundingClientRect();
  const r = el.getBoundingClientRect();
  const above = r.top - s.top - scroller.clientTop - inset;
  const below = r.bottom - s.bottom;
  const delta = block === "start" ? above : above < 0 ? above : below > 0 ? below : 0;
  if (Math.abs(delta) < 1) return;
  const reduce = window.matchMedia?.("(prefers-reduced-motion: reduce)").matches;
  scroller.scrollBy({ top: delta, behavior: reduce ? "auto" : "smooth" });
}
