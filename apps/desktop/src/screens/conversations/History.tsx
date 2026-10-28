/**
 * Owner: conversations. Every version of a line's memory, newest first (ADR 0014: each
 * applied summary is recorded). The newest is what the line remembers now.
 */
import type { CompactionView, TurnView } from "@ghost/client-ts";
import { History as HistoryIcon } from "lucide-react";
import { useState } from "react";

import { Badge, Button, Card, EmptyState, formatDateTime, relativeTime } from "../../ui";

const SOURCE: Record<string, string> = {
  auto: "Ghost compacted the transcript",
  manual: "You asked Ghost to compact",
  edit: "You edited the memory",
  operator: "You asked Ghost to compact",
};

export function sourceLabel(source: string): string {
  return SOURCE[source] ?? "Memory rewritten";
}

/**
 * What a version covers, in this line's own terms. `through_turn_id` is the store's global
 * turn id (shared by every line), so it is never shown: it is mapped onto the turns loaded
 * here. With the whole transcript loaded that is a count ("the first 12 turns"); when only
 * the newest turns are loaded, the covered turn's time. Null when it can't be said honestly.
 */
export function coverage(through: number | null, turns: TurnView[], whole: boolean): string | null {
  if (through == null) return null;
  const covered = turns.filter((t) => t.id <= through);
  if (covered.length === 0) return null;
  if (whole) {
    if (covered.length === turns.length) return covered.length === 1 ? "Covers the only turn so far" : `Covers all ${covered.length} turns so far`;
    return covered.length === 1 ? "Covers the first turn" : `Covers the first ${covered.length} turns`;
  }
  return `Covers everything said up to ${formatDateTime(covered[covered.length - 1].at)}`;
}

export interface HistoryProps {
  versions: CompactionView[];
  /** This line's turns as loaded, oldest first. */
  turns: TurnView[];
  /** Every turn of the line is in `turns` (the load wasn't capped). */
  whole: boolean;
  live: boolean;
  lineName: string;
  /** A settled line (ADR 0019): versions are read, never put back into memory. */
  readOnly?: boolean;
  /** Put an older version's summary in the memory editor, to save like any edit. */
  onUse: (c: CompactionView) => void;
}

export function History({ versions, turns, whole, live, lineName, readOnly, onUse }: HistoryProps) {
  if (versions.length === 0) {
    return (
      <EmptyState
        icon={<HistoryIcon />}
        title="No compactions yet"
        description={
          live
            ? "When the transcript gets long, Ghost folds older turns into memory. Use Compact now to do it at once. Every version is kept here."
            : `When the transcript gets long, Ghost folds older turns into memory and keeps every version here. Compact now works while a session is on ${lineName}.`
        }
      />
    );
  }
  const sorted = [...versions].sort((a, b) => b.at.localeCompare(a.at));
  return (
    <div className="stack gap-3">
      <p className="t-small t-2">
        {sorted.length} {sorted.length === 1 ? "version" : "versions"} of this agent’s memory, newest first.
        {!live && !readOnly && ` Compact now works while a session is on ${lineName}.`}
      </p>
      <Card list>
        {sorted.map((c, i) => (
          <Version key={c.id} c={c} current={i === 0} covers={coverage(c.through_turn_id, turns, whole)} onUse={readOnly ? undefined : () => onUse(c)} />
        ))}
      </Card>
    </div>
  );
}

function Version({ c, current, covers, onUse }: { c: CompactionView; current: boolean; covers: string | null; onUse?: () => void }) {
  const lines = c.summary.split("\n").filter(Boolean);
  const long = lines.length > 3 || c.summary.length > 360;
  const [open, setOpen] = useState(current);
  return (
    <article className="conv-version">
      <header className="conv-version__head">
        <span className="t-medium">{sourceLabel(c.source)}</span>
        {current && (
          <Badge size="sm" variant="outline">
            Current
          </Badge>
        )}
      </header>
      <p className={open || !long ? "conv-version__text" : "conv-version__text is-clamped"} data-user-content>
        {c.summary || <span className="t-3">Empty: the memory was cleared.</span>}
      </p>
      <footer className="conv-version__foot">
        <span className="conv-version__when t-small t-3">
          <time className="t-num" dateTime={c.at} title={relativeTime(c.at)}>
            {formatDateTime(c.at)}
          </time>
          {covers ? ` · ${covers[0].toLowerCase()}${covers.slice(1)}` : ""}
        </span>
        <span className="conv-version__actions">
          {long && (
            <Button size="sm" variant="ghost" aria-expanded={open} onClick={() => setOpen(!open)}>
              {open ? "Show less" : "Show all"}
            </Button>
          )}
          {!current && onUse && (
            <Button size="sm" variant="ghost" aria-label={`Use this version, from ${formatDateTime(c.at)}`} onClick={onUse}>
              Use this version
            </Button>
          )}
        </span>
      </footer>
    </article>
  );
}
