/**
 * Activity: every change Ghost made or asked to make, newest first, grouped by day. A row
 * opens in place to show how it was decided. Read-only on the phone.
 */
import type { AuditEntry } from "@ghost/client-ts";
import { autonomyLabel } from "@ghost/ui";
import { Power, ScrollText } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { EmptyState, Skeleton, cx, formatTime, relativeTime, statusLabel } from "../../ui";
import { didNotHappen, EntrySupport, EntryTitle, entryText, lastSeenAudit, rememberAudit, useAudit } from "./entries";
import { CompactBar, Scroller, TabBar, TouchButton, useLink } from "./parts";

const PAGE = 30;

export function Activity({ openId, cached }: { openId?: string | null; cached: { savedAt: string; audit: AuditEntry[] } }) {
  const [limit, setLimit] = useState(PAGE);
  const audit = useAudit({ limit });
  rememberAudit(audit.items);
  const link = useLink();
  const away = link === "offline" || link === "reconnecting";
  // Away from the desk: what this visit fetched, else what the phone saw last, else what it saved.
  const items = audit.items ?? (away ? lastSeenAudit() ?? cached.audit : null);
  const { total, error } = audit;
  const savedAt = !audit.items && !lastSeenAudit() && away ? cached.savedAt : null;
  const [open, setOpen] = useState<string | null>(openId ?? null);
  const groups = groupByDay(items ?? []);

  // Opened from a row elsewhere (#/mobile/activity?open=<id>): scroll that entry into the phone's own scroller.
  const scroller = useRef<HTMLDivElement>(null);
  const titleRef = useRef<HTMLHeadingElement>(null);
  const scrolled = useRef(false);
  useEffect(() => {
    if (!openId || scrolled.current || !items) return;
    const box = scroller.current;
    const row = box?.querySelector<HTMLElement>(`[data-entry="${CSS.escape(openId)}"]`);
    if (!box || !row) return;
    scrolled.current = true;
    const top = row.getBoundingClientRect().top - box.getBoundingClientRect().top + box.scrollTop - 48;
    box.scrollTo({ top: Math.max(0, top) });
    row.querySelector<HTMLElement>("button")?.focus({ preventScroll: true });
  }, [openId, items]);

  return (
    <div className="mob-screen mob-activity">
      <CompactBar title="Activity" watch={titleRef} />
      <Scroller className="mob-scroll--pad" ref={scroller}>
        <header className="mob-largehead">
          <h1 className="mob-largehead__title" ref={titleRef}>Activity</h1>
          <p className="mob-largehead__sub">Everything Ghost changed, or asked to.</p>
          {away && items && items.length > 0 && <p className="mob-stale-note">Showing what this phone saw last{savedAt ? <>, {relativeTime(savedAt)}</> : null}.</p>}
        </header>

        {items == null && !error ? (
          <div className="mob-list" aria-busy="true" aria-label="Loading">
            {[0, 1, 2, 3, 4].map((i) => (
              <div key={i} className="mob-row mob-row--skeleton">
                <Skeleton width={`${70 + ((i * 13) % 25)}%`} height={16} />
                <Skeleton width="40%" height={12} />
              </div>
            ))}
          </div>
        ) : items == null ? (
          <EmptyState
            className="mob-empty"
            icon={<Power />}
            title="Can’t reach your desk"
            description="Activity loads from Ghost on your workstation. The phone keeps trying."
            headingLevel="h2"
          />
        ) : items.length === 0 ? (
          <EmptyState
            className="mob-empty"
            icon={<ScrollText />}
            title="Nothing yet"
            description="When Ghost changes something, or asks to, it shows up here."
            headingLevel="h2"
          />
        ) : (
          <>
            {groups.map(([day, entries]) => (
              <section key={day} className="mob-day" aria-label={day}>
                <h2 className="mob-day__title">{day}</h2>
                <ul className="mob-list">
                  {entries.map((e) => (
                    <ActivityRow key={e.id} e={e} open={open === e.id} onToggle={() => setOpen(open === e.id ? null : e.id)} />
                  ))}
                </ul>
              </section>
            ))}
            {audit.items && items.length < total && (
              <div className="mob-more-wrap">
                <TouchButton variant="ghost" fullWidth onClick={() => setLimit((l) => l + PAGE)}>
                  Show older
                </TouchButton>
              </div>
            )}
          </>
        )}
      </Scroller>
      <TabBar current="activity" />
    </div>
  );
}

function ActivityRow({ e, open, onToggle }: { e: AuditEntry; open: boolean; onToggle: () => void }) {
  const { target, text } = entryText(e);
  const how = decidedBy(e);
  return (
    <li className={cx("mob-row", "mob-row--entry", open && "is-open")} data-entry={e.id}>
      <button type="button" className="mob-row__hit" aria-expanded={open} onClick={onToggle}>
        <span className="mob-row__text">
          <span className={cx("mob-row__title", !open && "mob-clamp-2", didNotHappen(e) && "mob-row__title--off")} data-user-content><EntryTitle e={e} text={text} /></span>
          <span className="mob-row__support">
            <EntrySupport e={e} target={target} />
          </span>
        </span>
      </button>
      {open && (
        <dl className="mob-detail">
          <div><dt>Status</dt><dd>{statusLabel(e.status)}</dd></div>
          <div><dt>Your rule</dt><dd>{autonomyLabel(e.autonomy)}</dd></div>
          {how && <div><dt>Decided by</dt><dd>{how}</dd></div>}
          <div><dt>When</dt><dd className="t-num">{formatTime(e.created_at)}</dd></div>
          <div><dt>Operation</dt><dd className="t-mono">{e.source}.{e.operation}</dd></div>
        </dl>
      )}
    </li>
  );
}

/** How a confirm-level change was settled, from its user_turn event, in plain words. */
function decidedBy(e: AuditEntry): string | null {
  const turn = e.events.find((x) => x.event === "user_turn");
  const d = (turn?.detail ?? {}) as { verdict?: string; classifier?: string; text?: string };
  if (e.status === "expired") return "Expired · Ghost left it alone";
  if (!turn) return null;
  const said = d.classifier === "ui" ? "the Confirm button" : d.text ? `you said “${d.text}”` : "your voice";
  return d.verdict === "negative" ? `You said no${d.text ? ` (“${d.text}”)` : ""}` : said.charAt(0).toUpperCase() + said.slice(1);
}

function groupByDay(items: AuditEntry[]): [string, AuditEntry[]][] {
  const out = new Map<string, AuditEntry[]>();
  const today = new Date();
  const key = (iso: string) => {
    const d = new Date(iso);
    const days = Math.round((Date.UTC(today.getFullYear(), today.getMonth(), today.getDate()) - Date.UTC(d.getFullYear(), d.getMonth(), d.getDate())) / 86400000);
    if (days === 0) return "Today";
    if (days === 1) return "Yesterday";
    return d.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
  };
  for (const e of items) {
    const k = key(e.created_at);
    out.set(k, [...(out.get(k) ?? []), e]);
  }
  return [...out.entries()];
}
