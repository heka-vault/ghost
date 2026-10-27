/**
 * Owner: apps. #/apps: the apps Ghost is connected to, then the catalog to add more from.
 * Everything a connected app offers runs (ADR 0034); a change is announced. What to block or
 * have Ghost ask first is set on each app's page, or for everything on Permissions.
 */
import type { AppView, CatalogEntryView, CatalogPage } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Blocks, CloudOff, ExternalLink, MoreHorizontal, Plus, RotateCw, Search, SearchX, ShieldCheck } from "lucide-react";
import { useCallback, useEffect, useRef, useState } from "react";

import { hrefFor } from "../../app/router";
import { useDesign } from "../../design/context";
import { Badge, Button, Card, EmptyState, IconButton, Input, ListRow, LiveStatus, Menu, Notice, Page, Section, Skeleton, Spinner, errorText, relativeTime, useLive } from "../../ui";
import { useAppActions } from "./actions";
import { AddOwnDialog } from "./AddOwnDialog";
import { AppIcon } from "./AppIcon";
import { ConnectDialog } from "./ConnectDialog";
import { sentence, summaryWords } from "./model";

const PAGE = 12;

/** A forced failure for design review (?fail=apps|catalog): development builds only, so the strings never ship. */
function useFail(): string | null {
  const design = useDesign();
  const [fail] = useState(() => (import.meta.env.DEV && design ? new URLSearchParams(window.location.search).get("fail") : null));
  return fail;
}

export function AppsIndex() {
  const { client, appsVersion } = useGhost();
  const fail = useFail();
  const [apps, setApps] = useState<AppView[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [connecting, setConnecting] = useState<CatalogEntryView | null>(null);
  const [addingOwn, setAddingOwn] = useState(false);
  const [catalogVersion, setCatalogVersion] = useState(0);

  const load = useCallback(async () => {
    try {
      if (import.meta.env.DEV && fail === "apps") throw new Error("Ghost didn’t answer GET /apps (mock).");
      setApps(await client.apps());
      setLoadError(null);
    } catch (e) {
      setLoadError(errorText(e));
    }
  }, [client, fail]);

  useEffect(() => {
    void load();
  }, [load, appsVersion]);

  const actions = useAppActions({ onChanged: () => void load() });

  const afterConnect = () => {
    void load();
    // The catalog's “Connected” marks follow what is connected now.
    setCatalogVersion((v) => v + 1);
  };

  return (
    <Page
      width="default"
      title="Apps"
      status={<LiveStatus />}
      description="What Ghost connects to. It uses everything an app offers and tells you what it changed. Block a tool or have Ghost ask first on the app’s page, or for everything on Permissions."
      actions={
        <Button variant="secondary" href={hrefFor("/permissions")} leadingIcon={<ShieldCheck />}>
          Permissions
        </Button>
      }
    >
      {loadError && (
        <Notice
          tone="danger"
          title={apps ? "Couldn’t refresh your apps" : "Couldn’t load your apps"}
          action={
            <Button size="sm" variant="secondary" onClick={() => void load()}>
              Try again
            </Button>
          }
          className="apps-notice"
        >
          {sentence(loadError)} Check that Ghost is running (<span className="t-mono">systemctl --user status ghostd</span>), then try again.
        </Notice>
      )}

      <Section id="apps-connected" title="Connected" description={apps && apps.length > 0 ? "Every agent and department reaches these. A department can narrow what its sessions may do in its own folder." : undefined}>
        {actions.problem && (
          <Notice tone="danger" title={actions.problem.title} onDismiss={actions.clearProblem} className="apps-notice">
            {actions.problem.text}
          </Notice>
        )}
        {!apps && !loadError ? (
          <ConnectedSkeleton />
        ) : apps && apps.length === 0 ? (
          <Card>
            <EmptyState
              icon={<Blocks />}
              title="No apps yet"
              description="Connect one below and Ghost can look things up in it and make changes for you, telling you what it changed."
            />
          </Card>
        ) : apps ? (
          <Card list>
            {apps.map((a) => (
              <AppRow key={a.name} app={a} actions={actions} />
            ))}
          </Card>
        ) : null}
      </Section>

      <Catalog key={catalogVersion} fail={fail} onPick={setConnecting} onAddOwn={() => setAddingOwn(true)} />

      {connecting && <ConnectDialog entry={connecting} onClose={() => setConnecting(null)} onConnected={afterConnect} />}
      <AddOwnDialog open={addingOwn} onClose={() => setAddingOwn(false)} onConnected={afterConnect} />
      {actions.dialog}
    </Page>
  );
}

/* ------------------------------------------------------------------ connected */

function AppRow({ app: a, actions }: { app: AppView; actions: ReturnType<typeof useAppActions> }) {
  const { online } = useLive();
  const busy = actions.busy === a.name;
  const line =
    a.sign_in_url ? (
      <span className="apps-row__state is-warn">Finish signing in in your browser</span>
    ) : a.state === "connected" && a.problem ? (
      <span className="apps-row__state is-warn" data-user-content>
        {summaryWords(a)} · {a.problem}
      </span>
    ) : a.state === "connected" ? (
      <span className="apps-row__state">{summaryWords(a)}</span>
    ) : a.state === "connecting" ? (
      <span className="apps-row__state">Connecting…</span>
    ) : a.state === "needs_sign_in" ? (
      <span className="apps-row__state is-warn">Needs you to sign in again</span>
    ) : (
      <span className="apps-row__state is-danger" data-user-content>
        Not connected{a.problem ? `: ${a.problem}` : ""}
      </span>
    );

  const primary =
    a.sign_in_url ? (
      <Button size="sm" variant="secondary" leadingIcon={<ExternalLink />} onClick={() => void actions.reopen(a)}>
        Open sign-in page
      </Button>
    ) : a.state === "needs_sign_in" ? (
      <Button size="sm" variant="secondary" loading={busy} disabled={!online} onClick={() => void actions.signIn(a)}>
        Sign in
      </Button>
    ) : a.state === "failed" || a.state === "stopped" ? (
      <Button size="sm" variant="secondary" leadingIcon={<RotateCw />} loading={busy} disabled={!online} onClick={() => void actions.signIn(a)}>
        Try again
      </Button>
    ) : a.state === "connecting" ? (
      <Spinner size={14} label={`Connecting ${a.title}`} />
    ) : null;

  return (
    <ListRow
      href={hrefFor(`/apps/${encodeURIComponent(a.name)}`)}
      leading={<AppIcon title={a.title} icon={a.icon} />}
      title={a.title}
      description={line}
      trailing={
        <span className="apps-row__actions">
          {primary}
          <Menu
            align="end"
            trigger={<IconButton size="sm" icon={<MoreHorizontal />} label={`More for ${a.title}`} />}
            items={[
              ...(a.kind === "sign_in" && a.state === "connected" ? [{ label: "Sign in again", onSelect: () => void actions.signIn(a), disabled: !online }] : []),
              { label: `Disconnect ${a.title}…`, danger: true, onSelect: () => void actions.disconnect(a), disabled: !online, reason: online ? undefined : "Ghost isn’t answering" },
            ]}
          />
        </span>
      }
    />
  );
}

function ConnectedSkeleton() {
  return (
    <Card list aria-busy="true" aria-label="Loading your apps">
      {[140, 96, 120].map((w, i) => (
        <div key={i} className="apps-skel-row">
          <Skeleton width={32} height={32} radius="var(--radius-md)" />
          <span className="apps-skel-row__text">
            <Skeleton width={w} height={12} />
            <Skeleton width={w + 80} height={10} />
          </span>
        </div>
      ))}
    </Card>
  );
}

/* -------------------------------------------------------------------- catalog */

function Catalog({ fail, onPick, onAddOwn }: { fail: string | null; onPick: (e: CatalogEntryView) => void; onAddOwn: () => void }) {
  const { client } = useGhost();
  const [query, setQuery] = useState("");
  const [search, setSearch] = useState("");
  const [page, setPage] = useState<CatalogPage | null>(null);
  const [entries, setEntries] = useState<CatalogEntryView[]>([]);
  const [loading, setLoading] = useState(true);
  const [more, setMore] = useState(false);
  const [error, setError] = useState<string | null>(null);
  /** The first copy can take a while (the daemon waits up to 15 seconds for it): say so after a moment. */
  const [slow, setSlow] = useState(false);
  const seq = useRef(0);

  // Debounce typing; Enter searches at once.
  useEffect(() => {
    const t = setTimeout(() => setSearch(query.trim()), 250);
    return () => clearTimeout(t);
  }, [query]);

  const load = useCallback(
    async (q: string) => {
      const n = ++seq.current;
      setLoading(true);
      setError(null);
      const slowTimer = setTimeout(() => n === seq.current && setSlow(true), 2500);
      try {
        if (import.meta.env.DEV && fail === "catalog") throw new Error("Ghost didn’t answer GET /catalog (mock).");
        const p = await client.catalog(q, 0, PAGE);
        if (n !== seq.current) return;
        setPage(p);
        setEntries(p.entries);
      } catch (e) {
        if (n === seq.current) setError(errorText(e));
      } finally {
        clearTimeout(slowTimer);
        if (n === seq.current) {
          setLoading(false);
          setSlow(false);
        }
      }
    },
    [client, fail],
  );

  useEffect(() => {
    void load(search);
  }, [load, search]);

  // The first copy hasn't arrived yet: look again in a few seconds.
  useEffect(() => {
    if (!page?.refreshing || page.entries.length > 0) return;
    const t = setTimeout(() => void load(search), 4000);
    return () => clearTimeout(t);
  }, [page, load, search]);

  const showMore = async () => {
    setMore(true);
    try {
      const p = await client.catalog(search, entries.length, PAGE);
      setEntries((e) => [...e, ...p.entries.filter((x) => !e.some((y) => y.name === x.name))]);
      setPage(p);
    } catch (e) {
      setError(errorText(e));
    } finally {
      setMore(false);
    }
  };

  const first = loading && !page;
  const waitingForCopy = !!page && page.refreshing && page.entries.length === 0 && !search;

  return (
    <Section
      id="apps-catalog"
      title="Add an app"
      description="From the official registry of apps that work with assistants like Ghost. Their descriptions are the makers’ own words."
    >
      <div className="apps-search">
        <Input
          type="search"
          aria-label="Search apps"
          placeholder="Search apps, like Slack or Postgres…"
          value={query}
          leading={<Search size={14} aria-hidden />}
          trailing={loading && page ? <Spinner size={14} label="Searching" /> : undefined}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") setSearch(query.trim());
            if (e.key === "Escape" && query) {
              e.preventDefault();
              setQuery("");
            }
          }}
        />
      </div>

      {page?.problem && !error && (
        <Notice tone="warn" title="Couldn’t refresh the list" className="apps-notice">
          {sentence(page.problem)} {page.fetched_at ? `This copy is from ${relativeTime(page.fetched_at)}.` : ""}
        </Notice>
      )}

      {error ? (
        <Card>
          <EmptyState
            size="compact"
            icon={<CloudOff />}
            title="Couldn’t get the list of apps"
            description={sentence(error)}
            actions={
              <Button size="sm" variant="secondary" onClick={() => void load(search)}>
                Try again
              </Button>
            }
          />
        </Card>
      ) : first || waitingForCopy ? (
        <div aria-busy="true">
          <p className="apps-wait t-small t-2" aria-live="polite">
            {slow || waitingForCopy ? "Ghost is getting its first copy of the list. That can take about 15 seconds." : "Getting the list of apps…"}
          </p>
          <div className="apps-grid" aria-hidden>
            {Array.from({ length: 6 }, (_, i) => (
              <div key={i} className="apps-card apps-card--skeleton">
                <Skeleton width={32} height={32} radius="var(--radius-md)" />
                <Skeleton width="50%" height={12} />
                <Skeleton width="90%" height={10} />
                <Skeleton width="70%" height={10} />
              </div>
            ))}
          </div>
        </div>
      ) : entries.length === 0 ? (
        <Card>
          <EmptyState
            size="compact"
            icon={<SearchX />}
            title={search ? `No app called “${search}” in the list` : "The list is empty"}
            description="If you know its address, or have its config from another app, you can add it yourself."
            actions={
              <Button size="sm" variant="secondary" leadingIcon={<Plus />} onClick={onAddOwn}>
                Add your own…
              </Button>
            }
          />
        </Card>
      ) : (
        <>
          <ul className={loading ? "apps-grid is-stale" : "apps-grid"} aria-label={search ? `Apps matching ${search}` : "Apps to connect"}>
            {entries.map((e) => (
              <li key={e.name}>
                <CatalogCard entry={e} onPick={() => onPick(e)} />
              </li>
            ))}
          </ul>
          {page && entries.length < page.total && (
            <div className="apps-more">
              <Button variant="secondary" loading={more} onClick={() => void showMore()}>
                Show more
              </Button>
              <span className="t-small t-3 t-num">
                {entries.length} of {page.total}
              </span>
            </div>
          )}
        </>
      )}

      {!error && entries.length > 0 && (
        <p className="apps-own t-small t-2">
          Not in the list?{" "}
          <Button variant="ghost" size="sm" onClick={onAddOwn}>
            Add your own…
          </Button>
        </p>
      )}
    </Section>
  );
}

/** One app in the catalog. Its title and description are the registry's text: plain text, never markup. */
function CatalogCard({ entry: e, onPick }: { entry: CatalogEntryView; onPick: () => void }) {
  const kinds = [...new Set(e.options.map((o) => o.kind))];
  const how = kinds.map((k) => ({ sign_in: "Sign in", key: "API key", local: "Runs here" })[k]).join(" · ");
  return (
    <button type="button" className="apps-card" onClick={onPick} aria-label={`${e.title}${e.connected.length ? ", connected" : ""}. ${e.description}`}>
      <span className="apps-card__head">
        <AppIcon title={e.title} icon={e.icon} />
        {e.connected.length > 0 && (
          <Badge size="sm" tone="ok" dot>
            Connected
          </Badge>
        )}
      </span>
      <span className="apps-card__title" data-user-content>
        {e.title}
      </span>
      <span className="apps-card__desc" data-user-content>
        {e.description}
      </span>
      <span className="apps-card__meta">{how}</span>
    </button>
  );
}
