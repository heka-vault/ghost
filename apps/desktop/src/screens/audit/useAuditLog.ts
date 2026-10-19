/**
 * Owner: audit. Loads the audit log a page at a time and keeps what's loaded fresh.
 *
 * Pages of PAGE rows, newest first, appended by "Show older". Whenever `signal` changes (a
 * change started or stopped waiting, a write settled, a line was edited, a turn landed) it
 * re-reads the newest rows and merges them by id, so statuses settle in place and new
 * changes appear at the top, marked new for a moment. A refresh reads only the newest PAGE,
 * unless an older loaded row can still change (waiting or writing): then it reads down to
 * that row. The loaded rows stay a contiguous prefix of the
 * log, so "Show older" keeps using the loaded count as its offset.
 *
 * A failed refresh keeps the rows already on screen (reconnecting never clears the log).
 * A failed "Show older" is its own error, shown by the button that failed.
 */
import type { AuditEntry, GhostClient } from "@ghost/client-ts";
import { useCallback, useEffect, useRef, useState } from "react";

export const PAGE = 50;
/** The API's limit per request; also how deep an automatic search reads before it hands back to "Show older". */
export const MAX_LOADED = 500;
const MAX = MAX_LOADED;

export interface AuditLog {
  items: AuditEntry[];
  total: number;
  /** True until the first page answers (or fails). */
  loading: boolean;
  loadingMore: boolean;
  /** The last refresh failed (the first load, or a live refresh). */
  error: unknown;
  /** The last "Show older" failed. */
  moreError: unknown;
  /** Ids that arrived with the last live refresh. */
  fresh: Set<string>;
  /** When the log last answered. */
  updatedAt: number | null;
  loadMore: () => void;
  retry: () => void;
}

/** How deep a live refresh must read so every row that can still change is covered. */
function depth(loaded: AuditEntry[]): number {
  let last = -1;
  for (let i = PAGE; i < loaded.length; i++) {
    const e = loaded[i];
    if (e.status === "pending" || e.status === "claimed") last = i;
  }
  return Math.min(MAX, Math.max(PAGE, last + 1));
}

export function useAuditLog(client: GhostClient, signal: string, opts: { fail?: boolean; failMore?: boolean } = {}): AuditLog {
  const [items, setItems] = useState<AuditEntry[]>([]);
  const [total, setTotal] = useState(0);
  const [loading, setLoading] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);
  const [error, setError] = useState<unknown>(null);
  const [moreError, setMoreError] = useState<unknown>(null);
  const [fresh, setFresh] = useState<Set<string>>(() => new Set());
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const loaded = useRef<AuditEntry[]>([]);
  const seq = useRef(0);

  const fetchPage = useCallback(
    (limit: number, offset: number) => {
      if (opts.fail || (opts.failMore && offset > 0)) return Promise.reject(new Error("Ghost didn’t answer the audit request. It may be restarting."));
      return client.audit({ limit, offset });
    },
    [client, opts.fail, opts.failMore],
  );

  const refresh = useCallback(
    async (live: boolean) => {
      const n = seq.current + 1;
      seq.current = n;
      try {
        const page = await fetchPage(depth(loaded.current), 0);
        if (seq.current !== n) return;
        const before = loaded.current;
        const known = new Set(before.map((e) => e.id));
        // The page is the newest rows; everything loaded that it didn't cover is older.
        const ids = new Set(page.items.map((e) => e.id));
        const next = [...page.items, ...before.filter((e) => !ids.has(e.id))];
        loaded.current = next;
        setItems(next);
        setTotal(Math.max(page.total, next.length));
        setError(null);
        setUpdatedAt(Date.now());
        if (live && before.length) setFresh(new Set(page.items.filter((e) => !known.has(e.id)).map((e) => e.id)));
      } catch (e) {
        if (seq.current === n) setError(e);
      } finally {
        if (seq.current === n) setLoading(false);
      }
    },
    [fetchPage],
  );

  // The signal the loaded rows reflect. A live refresh runs only when the signal moves past
  // it, and never while the first load is in flight: the shared state settles on mount
  // (turns, conversation versions), and a refresh then would supersede the first load and
  // hold the skeleton for a second round trip. If the signal moved during the first load,
  // one refresh follows as soon as it lands.
  const seen = useRef(signal);
  const latest = useRef(signal);
  latest.current = signal;

  // First load.
  useEffect(() => {
    loaded.current = [];
    seen.current = latest.current;
    setLoading(true);
    void refresh(false);
  }, [refresh]);

  // Live: re-read shortly after anything that can change the log.
  useEffect(() => {
    if (loading || signal === seen.current) return;
    const t = setTimeout(() => {
      seen.current = signal;
      void refresh(true);
    }, 120);
    return () => clearTimeout(t);
  }, [signal, loading, refresh]);

  const loadMore = useCallback(async () => {
    setLoadingMore(true);
    setMoreError(null);
    try {
      const page = await fetchPage(PAGE, loaded.current.length);
      const ids = new Set(loaded.current.map((e) => e.id));
      const next = [...loaded.current, ...page.items.filter((e) => !ids.has(e.id))];
      loaded.current = next;
      setItems(next);
      setTotal(Math.max(page.total, next.length));
      setUpdatedAt(Date.now());
    } catch (e) {
      setMoreError(e);
    } finally {
      setLoadingMore(false);
    }
  }, [fetchPage]);

  const retry = useCallback(() => {
    if (!loaded.current.length) setLoading(true);
    void refresh(false);
  }, [refresh]);

  return { items, total, loading, loadingMore, error, moreError, fresh, updatedAt, loadMore: () => void loadMore(), retry };
}
