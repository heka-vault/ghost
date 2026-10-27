/**
 * Owner: waiting. What needs the operator (ADR 0029): one list across every watch, every
 * webhook and Ghost itself, the changes waiting for a yes included. Each item says where it
 * came from, how long it has waited and whether the operator has heard about it; the
 * actions are the operator's own bookkeeping (got it, later) and, for a change, the same
 * Confirm the session has (ADR 0013, ADR 0030).
 *
 * Titles come from outside systems: they are shown as text, never as markup.
 */
import type { GhostClient, WaitingItemView, WaitingView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { BellOff, Check, ExternalLink, Eye, GitPullRequestArrow, Inbox, ShieldAlert, Webhook } from "lucide-react";
import { useCallback, useEffect, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { Badge, Button, Card, EmptyState, IconButton, ListRow, LiveStatus, Menu, Notice, Page, Skeleton, errorText, relativeTime, useToast } from "../../ui";
import "./waiting.css";

const STATE: Record<WaitingItemView["state"], { label: string; tone: "warn" | "info" | "neutral" }> = {
  new: { label: "New", tone: "warn" },
  delivered: { label: "Sent, not heard", tone: "info" },
  acknowledged: { label: "Heard", tone: "neutral" },
  snoozed: { label: "Later", tone: "neutral" },
};

/** Where an item came from, in plain words: a watch, a webhook, a change, Ghost itself. */
export function sourceLabel(item: WaitingItemView): string {
  const [kind, rest] = item.source.includes(":") ? item.source.split(/:(.*)/s, 2) : [item.source, ""];
  if (kind === "watch") return `Watch ${rest}`;
  if (kind === "hook") return `Webhook ${rest}`;
  if (kind === "gate") return "A change";
  return "Ghost";
}

function SourceIcon({ item }: { item: WaitingItemView }) {
  if (item.change) return <GitPullRequestArrow aria-hidden />;
  if (item.source.startsWith("hook:")) return <Webhook aria-hidden />;
  if (item.source.startsWith("watch:")) return <Eye aria-hidden />;
  return <ShieldAlert aria-hidden />;
}

/**
 * What an action answered, when it didn't do what was asked. The daemon answers 200 with
 * `ok: false` (an item already gone, a change no longer waiting) or, for a confirmed change,
 * an outcome whose status is `failed` or `partial`: those are failures to show, not "Done".
 */
export function actionFailure(r: unknown): { title: string; text: string; tone: "danger" | "warn" } | null {
  if (!r || typeof r !== "object") return null;
  const v = r as { ok?: boolean; detail?: string | null; status?: string; error?: string | null; summary?: string };
  if (v.ok === false) return { title: "That didn’t work", text: v.detail || "It may have changed meanwhile; the list is up to date now.", tone: "danger" };
  if (v.status === "failed") return { title: "The change didn’t go through", text: v.error || v.summary || "", tone: "danger" };
  if (v.status === "partial") return { title: "Only part of the change went through", text: v.error || v.summary || "", tone: "warn" };
  return null;
}

/** Minutes until 9:00 tomorrow, for "Tomorrow morning". */
function untilTomorrowMorning(): number {
  const t = new Date();
  t.setDate(t.getDate() + 1);
  t.setHours(9, 0, 0, 0);
  return Math.max(1, Math.round((t.getTime() - Date.now()) / 60000));
}

export default function WaitingScreen(_: RouteProps) {
  const { client, status, waitingVersion } = useGhost();
  const toast = useToast();
  const [view, setView] = useState<WaitingView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [busy, setBusy] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setView(await client.waiting());
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [client]);

  // Load, and again whenever the daemon says the list changed or a session ends.
  useEffect(() => {
    void load();
  }, [load, waitingVersion, status?.session_id]);

  const act = async (key: string, run: () => Promise<unknown>, done?: string) => {
    setBusy(key);
    try {
      const failed = actionFailure(await run());
      if (failed) toast({ title: failed.title, description: failed.text || undefined, tone: failed.tone });
      else if (done) toast({ title: done });
      await load();
    } catch (e) {
      toast({ title: "That didn’t work", description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };

  const items = view?.items ?? [];
  const toHear = view?.count ?? 0;

  return (
    <Page
      title="What needs you"
      status={<LiveStatus />}
      description="Everything Ghost is keeping track of for you: what your watches and webhooks found, and changes waiting for your yes. “Got it” stops the reminders until something changes; nothing here writes anywhere."
    >
      {error && (
        <Notice
          tone="danger"
          title={view ? "Couldn’t refresh the list" : "Couldn’t load the list"}
          action={
            <Button size="sm" onClick={() => void load()}>
              Try again
            </Button>
          }
          className="wait-notice"
        >
          {error}
        </Notice>
      )}

      {!view && !error ? (
        <Card list aria-busy="true" aria-label="Loading">
          {[0, 1, 2].map((i) => (
            <div key={i} className="wait-skel">
              <Skeleton width="55%" height={14} />
              <Skeleton width="30%" height={10} />
            </div>
          ))}
        </Card>
      ) : view && items.length === 0 ? (
        <Card>
          <EmptyState
            icon={<Inbox />}
            title="Nothing needs you"
            description="When a watch or a webhook finds something, or a change waits for your yes, it shows up here, and Ghost tells you the way your routes say."
          />
        </Card>
      ) : (
        <>
          <p className="wait-summary t-small t-2" aria-live="polite">
            {toHear === 0 ? "You’ve heard about everything here." : toHear === 1 ? "One thing you haven’t heard about yet." : `${toHear} things you haven’t heard about yet.`}
          </p>
          <Card list>
            <ul className="wait-list" role="list" aria-label="What needs you">
              {items.map((item) => (
                <li key={item.key}>
                  <ListRow
                    align="start"
                    leading={
                      <span className="wait-icon">
                        <SourceIcon item={item} />
                      </span>
                    }
                    title={<span className="wait-title">{item.title}</span>}
                    description={
                      <span className="wait-meta">
                        <span>{sourceLabel(item)}</span>
                        {item.department && <span>{item.department}</span>}
                        <span>first seen {relativeTime(item.first_seen)}</span>
                        {item.delivered_via && item.state === "delivered" && <span>sent by {item.delivered_via}</span>}
                        {item.change && <span>until {new Date(item.change.expires_at).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}</span>}
                      </span>
                    }
                    meta={
                      <Badge size="sm" tone={STATE[item.state].tone} dot={item.state === "new"}>
                        {STATE[item.state].label}
                      </Badge>
                    }
                    trailing={<Actions item={item} busy={busy === item.key} act={act} client={client} />}
                  />
                </li>
              ))}
            </ul>
          </Card>
        </>
      )}
    </Page>
  );
}

type Act = (key: string, run: () => Promise<unknown>, done?: string) => Promise<void>;

/** A change: Cancel and Confirm, as in the session. Anything else: Got it, and Later. */
function Actions({ item, busy, act, client }: { item: WaitingItemView; busy: boolean; act: Act; client: GhostClient }) {
  const later = (minutes: number, label: string) => ({
    label,
    onSelect: () => void act(item.key, () => client.snooze(item.key, minutes), "Snoozed"),
  });
  const change = item.change;
  return (
    <span className="wait-actions">
      {item.link && <IconButton size="sm" variant="ghost" label="Open" icon={<ExternalLink />} href={item.link} />}
      {change ? (
        <>
          <Button size="sm" variant="secondary" loading={busy} onClick={() => void act(item.key, () => client.cancelProposal(change.id), "Cancelled; nothing changed")}>
            Cancel
          </Button>
          <Button size="sm" leadingIcon={<Check />} loading={busy} onClick={() => void act(item.key, () => client.confirmProposal(change.id), "Done")}>
            Confirm
          </Button>
        </>
      ) : (
        <>
          {item.state !== "acknowledged" && (
            <Button size="sm" variant="secondary" loading={busy} onClick={() => void act(item.key, () => client.acknowledge(item.key))}>
              Got it
            </Button>
          )}
          <Menu
            align="end"
            aria-label="Remind me later"
            trigger={<IconButton size="sm" variant="ghost" label="Later" icon={<BellOff />} />}
            items={[later(30, "In 30 minutes"), later(120, "In 2 hours"), later(untilTomorrowMorning(), "Tomorrow morning")]}
          />
        </>
      )}
    </span>
  );
}
