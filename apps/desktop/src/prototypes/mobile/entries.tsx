/** How the phone shows an audit entry: what happened in plain words, the issue as mono detail. */
import type { AuditEntry } from "@ghost/client-ts";
import { didNotHappen, splitSummary, useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

import { hrefFor } from "../../app/router";
import { Badge, cx, relativeTime, statusLabel, statusTone } from "../../ui";
import { Row } from "./parts";

/** The target as the row shows it: an issue ref in mono, a line's name in regular text. */
export function TargetText({ target, conversation, className }: { target: string; conversation: boolean; className?: string }) {
  return <span className={cx(!conversation && "t-mono", "mob-nowrap", className)}>{target}</span>;
}

/** The sentence for a row: the gate's own summary, as the desktop audit shows it; the status badge says whether it happened. */
export function entryText(e: AuditEntry): { target: string; text: string } {
  return splitSummary(e);
}

/** The Activity route with this entry opened and scrolled into view. */
export function entryHref(id: string): string {
  return hrefFor(`/mobile/activity?open=${encodeURIComponent(id)}`);
}

/**
 * A row's status, only when it says something. Done is the normal case, so it shows nothing.
 * Every other status is the same Badge the desktop audit uses, through statusLabel() and
 * statusTone(): neutral for Canceled and Expired; warn and danger for Waiting,
 * Changed first and Failed, where the color is the news.
 */
export function StatusMark({ e }: { e: Pick<AuditEntry, "status"> }) {
  if (e.status === "executed") return null;
  return (
    <Badge tone={statusTone(e.status)} dot size="sm" className="mob-status-badge">
      {statusLabel(e.status)}
    </Badge>
  );
}

/** A row's sentence: what Ghost did or proposed, in the gate's words. Any status other than Done is a badge on the support line (see EntrySupport). */
export function EntryTitle({ text }: { e: AuditEntry; text: string }) {
  return <>{text}</>;
}

/** The support line: the status badge (when not done), then issue (or line) · when. */
export function EntrySupport({ e, target }: { e: AuditEntry; target: string }) {
  return (
    <>
      <StatusMark e={e} />
      <TargetText target={target} conversation={e.target.startsWith("conversation:")} />
      <span aria-hidden> · </span>
      <span className="mob-nowrap">{relativeTime(e.created_at)}</span>
    </>
  );
}

export function EntryRow({ e, href }: { e: AuditEntry; href?: string }) {
  const { target, text } = entryText(e);
  return (
    <Row
      href={href ?? entryHref(e.id)}
      userContent
      title={<span className={cx("mob-clamp-2", didNotHappen(e) && "mob-row__title--off")}><EntryTitle e={e} text={text} /></span>}
      support={<EntrySupport e={e} target={target} />}
    />
  );
}

/**
 * Audit entries from the daemon, refetched when a change settles (outcome, pending). Keeps
 * the last good page when a request fails, so a dropped connection shows stale data, not a
 * blank screen.
 */
export function useAudit(query: { limit: number; session_id?: string | null }) {
  const { client, lastEvent, connected } = useGhost();
  const [items, setItems] = useState<AuditEntry[] | null>(null);
  const [total, setTotal] = useState(0);
  const [error, setError] = useState<unknown>(null);
  const [attempt, setAttempt] = useState(0);
  const [loading, setLoading] = useState(true);
  const key = `${query.limit}:${query.session_id ?? ""}`;
  const bump = lastEvent && (lastEvent.type === "outcome" || lastEvent.type === "pending" || lastEvent.type === "status") ? lastEvent : null;
  useEffect(() => {
    let live = true;
    setLoading(true);
    client
      .audit({ limit: query.limit, ...(query.session_id ? { session_id: query.session_id } : {}) })
      .then((p) => {
        if (!live) return;
        setItems(p.items);
        setTotal(p.total);
        setError(null);
      })
      .catch((e) => live && setError(e))
      .finally(() => live && setLoading(false));
    return () => {
      live = false;
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [client, key, bump, connected, attempt]);
  return { items, total, error, loading, retry: () => setAttempt((a) => a + 1) };
}

/**
 * The phone's memory of the last audit page it saw, across screens, so Home and Activity
 * can show it while the desk is unreachable (a fresh mount can't fetch anything then).
 */
let lastSeen: AuditEntry[] | null = null;
export function rememberAudit(items: AuditEntry[] | null) {
  if (items && items.length) lastSeen = items;
}
export function lastSeenAudit(): AuditEntry[] | null {
  return lastSeen;
}

export { didNotHappen, neverHappened, pastTense, shortTarget, splitSummary } from "@ghost/ui";
