/** How the phone shows an audit entry: what happened in plain words, the issue as mono detail. */
import type { AuditEntry } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Undo2 } from "lucide-react";
import { useEffect, useState } from "react";

import { hrefFor } from "../../app/router";
import { Badge, cx, lineName, relativeTime, statusLabel, statusTone } from "../../ui";
import { Row } from "./parts";

type Summarized = Pick<AuditEntry, "summary" | "target"> & { operation?: string; undo_of?: string | null };

/**
 * The gate names an undo `undo:<op>` and links it with `undo_of`; its summary is the inverted
 * steps (“On infra#77: delete the comment I added; labels: …”), never the word “Undo”. Any of
 * the three marks the row as an undo.
 */
export function isUndo(e: Summarized): boolean {
  return !!e.undo_of || (e.operation ?? "").startsWith("undo:") || /^Undo:\s*/.test(e.summary);
}

/**
 * “On ghost-mock/app#208: approved, with …” → { target: "ghost-mock/app#208", text: "Approved, with …" }.
 * A change to a line (persona, voice) has `conversation: true` and its line's name as the
 * target: “the Standup line”, never a slug in mono. An undo reads as what it reversed, never as
 * the steps it ran: “Undid the approval”, not “Delete the comment I added”.
 */
export function splitSummary(e: Summarized): { target: string; text: string; conversation: boolean } {
  const prefix = /^Undo:\s*/.exec(e.summary);
  const undo = isUndo(e);
  const rest = prefix ? e.summary.slice(prefix[0].length) : e.summary;
  const m = /^On ([^:]+):\s*(.*)$/s.exec(rest);
  const body = cap(m ? m[2] : rest);
  const conversation = e.target.startsWith("conversation:");
  const target = conversation ? `${lineName(e.target.slice("conversation:".length))} line` : m ? m[1] : e.target;
  if (!undo) return { target, text: body, conversation };
  // An undo says what it reversed, as a noun when the operation has one: “Undid the approval”.
  const noun = UNDONE_NOUN[(e.operation ?? "").replace(/^undo:/, "")];
  return { target, text: noun ? `Undid the ${noun}` : `Undid this: ${body}`, conversation };
}

/** The target as the row shows it: an issue ref in mono, a line's name in regular text. */
export function TargetText({ target, conversation, className }: { target: string; conversation: boolean; className?: string }) {
  return <span className={cx(!conversation && "t-mono", "mob-nowrap", className)}>{target}</span>;
}

const cap = (t: string) => t.charAt(0).toUpperCase() + t.slice(1);

const UNDONE_NOUN: Record<string, string> = {
  approve: "approval",
  close: "close",
  reopen: "reopen",
  comment: "comment",
  request_changes: "change request",
  set_labels: "label change",
  add_labels: "label change",
  remove_labels: "label change",
  assign: "assignment",
  set_milestone: "milestone",
  lock: "lock",
  remember: "note",
  set_voice: "voice change",
  set_persona: "persona change",
  create: "new line",
};

/** The change never took effect: nothing on GitHub moved. Undone and partial did happen (then or in part). */
export function neverHappened(e: Pick<AuditEntry, "status">): boolean {
  return e.status === "cancelled" || e.status === "expired" || e.status === "stale" || e.status === "failed" || e.status === "pending";
}

/** The sentence for a row: the gate's own summary, as the desktop audit shows it; the status badge says whether it happened. */
export function entryText(e: AuditEntry): { target: string; text: string } {
  return splitSummary(e);
}

/** An operation in the past tense, for a result: “Closed”, “Approved”, “Labeled”. */
export function pastTense(operation: string): string {
  const known: Record<string, string> = {
    close: "Closed",
    reopen: "Reopened",
    approve: "Approved",
    comment: "Commented",
    set_labels: "Labeled",
    add_labels: "Labeled",
    remove_labels: "Labeled",
    assign: "Assigned",
    unassign: "Unassigned",
    set_milestone: "Milestone set",
    lock: "Locked",
  };
  return known[operation] ?? "Done";
}

/** The short name of an issue for a button label: “ghost-mock/app#208” → “app#208”. */
export function shortTarget(target: string): string {
  return target.split("/").pop() ?? target;
}

/** The Activity route with this entry opened and scrolled into view. */
export function entryHref(id: string): string {
  return hrefFor(`/mobile/activity?open=${encodeURIComponent(id)}`);
}

/**
 * A row's status, only when it says something. Done is the normal case, so it shows nothing.
 * Every other status is the same Badge the desktop audit uses, through statusLabel() and
 * statusTone(): neutral for Canceled, Expired and Undone; warn and danger for Waiting,
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

/**
 * The change never happened (or no longer stands): canceled, expired, undone, changed first,
 * failed. Its sentence steps down to --text-2 so it doesn't read as done. Waiting stays --text-1.
 */
export function didNotHappen(e: Pick<AuditEntry, "status">): boolean {
  return e.status !== "executed" && e.status !== "pending";
}

/**
 * A row's sentence: what Ghost did or proposed, in the gate's words. An undo that ran leads
 * with the undo arrow, so it reads as a reversal, not as a fresh change: “↶ Undid the approval”.
 * Any status other than Done is a badge on the support line (see EntrySupport).
 */
export function EntryTitle({ e, text }: { e: AuditEntry; text: string }) {
  if (e.status === "executed" && isUndo(e)) {
    return (
      <>
        <span className="mob-status mob-status--undo"><Undo2 size={14} aria-hidden /></span>
        {text}
      </>
    );
  }
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
