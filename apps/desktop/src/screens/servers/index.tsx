/**
 * Owner: servers. The MCP servers the policy file declares (ADR 0022): each one, whether
 * Ghost reached it, what it offers the model to look up, the writes the operator declared
 * for the gate, and what it has that no declaration covers. Nothing here is a control: a
 * server is declared, scoped and leveled in the policy file, so the page links there.
 *
 * URL states (query before the hash):
 *   show=<name>    the selected server (kept in sync as you pick one: reload and links keep it)
 *   fail=servers   mock only: GET /mcp fails
 */
import type { McpServerInfo, SourceInfo } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { Code, Server } from "lucide-react";
import { type KeyboardEvent, useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { useDesign } from "../../design/context";
import { Button, Card, EmptyState, ListRow, LiveStatus, Notice, Page, Skeleton, SkeletonText, errorText, scrollIntoPanel } from "../../ui";
import { Detail } from "./Detail";
import { humanize, policyFileHref, summary, transportWord } from "./model";
import "./servers.css";

const params = () => new URLSearchParams(window.location.search);

/** Change the query (before the hash) in place, without adding a history entry. */
function writeQuery(mutate: (q: URLSearchParams) => void) {
  const q = params();
  mutate(q);
  const search = q.toString();
  window.history.replaceState(window.history.state, "", `${window.location.pathname}${search ? `?${search}` : ""}${window.location.hash}`);
}

/** Keep `show=<name>` in step with the selection. */
const writeShow = (name: string | null) => writeQuery((q) => (name ? q.set("show", name) : q.delete("show")));

export default function ServersScreen(_: RouteProps) {
  const { client, status } = useGhost();
  const design = useDesign();
  const init = useRef(params()).current;
  // A forced failure for design review (?fail=servers): development builds only, so the string never ships.
  const fail = import.meta.env.DEV && design ? init.get("fail") : null;

  const [servers, setServers] = useState<McpServerInfo[] | null>(null);
  /** What the gate built from each connected server: the words a write was declared with. */
  const [sources, setSources] = useState<SourceInfo[] | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  const [selected, setSelected] = useState<string | null>(init.get("show"));

  const load = useCallback(async () => {
    const [m, s] = await Promise.allSettled([
      import.meta.env.DEV && fail === "servers" ? Promise.reject(new Error("Ghost didn’t answer GET /mcp (mock).")) : client.mcp(),
      client.sources(),
    ]);
    if (m.status === "fulfilled") {
      setServers(m.value);
      setLoadError(null);
    } else setLoadError(errorText(m.reason));
    if (s.status === "fulfilled") setSources(s.value);
  }, [client, fail]);

  // Load, and again whenever the set of connected servers changes (a server came up or went away).
  const connectedKey = (status?.servers ?? []).join("\n");
  useEffect(() => {
    void load();
  }, [load, connectedKey]);

  const current = servers?.find((s) => s.name === selected) ?? servers?.[0] ?? null;
  const source = (current && sources?.find((s) => s.name === current.name)) ?? null;

  // Keep the selection in the URL so reload and links land on it; tidy up on the way out.
  useEffect(() => {
    if (current) writeShow(current.name);
  }, [current?.name]);
  useEffect(() => () => writeShow(null), []);

  // A new pick while the panel is scrolled down would land mid-way through the next server,
  // its name under the compact bar. Bring its header back, level with the list; never move
  // when the header is already in view.
  const headerAnchor = useRef<HTMLSpanElement>(null);
  const shown = useRef<string | null>(null);
  useLayoutEffect(() => {
    const name = current?.name ?? null;
    const previous = shown.current;
    shown.current = name;
    if (!previous || !name || previous === name) return;
    const anchor = headerAnchor.current;
    const scroller = anchor?.closest(".shell__main");
    if (!anchor || !scroller) return;
    if (anchor.getBoundingClientRect().top < scroller.getBoundingClientRect().top - 1) scrollIntoPanel(anchor, { block: "start", smooth: false });
  }, [current?.name]);

  return (
    <Page
      width="wide"
      title="Servers"
      status={<LiveStatus />}
      description="The MCP servers Ghost connects to, declared in your policy file. What they can look up is offered to the model as it is; a write is offered only once you declare it, and then goes through the gate at the level you set."
      actions={
        <Button variant="secondary" href={policyFileHref()} leadingIcon={<Code />}>
          Edit the policy file
        </Button>
      }
    >
      {loadError && (
        <Notice
          tone="danger"
          title={servers ? "Couldn’t refresh servers" : "Couldn’t load servers"}
          action={
            <Button size="sm" onClick={() => void load()}>
              Try again
            </Button>
          }
          className="srv-notice"
        >
          <p className="srv-notice__p">{loadError}</p>
          <p className="srv-notice__p">
            Check that Ghost is running (<span className="t-mono">systemctl --user status ghostd</span>), then try again.
          </p>
        </Notice>
      )}

      {!servers && !loadError ? (
        <LoadingPanes />
      ) : servers && servers.length === 0 ? (
        <Card>
          <EmptyState
            icon={<Server />}
            title="No servers yet"
            description={
              <>
                Declare one in <span className="t-mono">~/.config/ghost/policy.toml</span> under <span className="t-mono">[mcp.&lt;name&gt;]</span>, with the
                command Ghost runs or the URL it connects to. Ghost offers what it can look up to the model; a write is offered once you declare it.
              </>
            }
          />
        </Card>
      ) : servers ? (
        <div className="srv-panes">
          <ServerList servers={servers} current={current?.name ?? null} onSelect={setSelected} />
          <div className="srv-pane">{current && <Detail key={current.name} server={current} source={source} headerAnchor={headerAnchor} />}</div>
        </div>
      ) : null}
    </Page>
  );
}

/**
 * The declared servers as a listbox of rows: one tab stop (the selected row), arrows and
 * Home/End move and select. A row says what it offers; one Ghost couldn’t reach says so.
 */
function ServerList({ servers, current, onSelect }: { servers: McpServerInfo[]; current: string | null; onSelect: (name: string) => void }) {
  const ref = useRef<HTMLDivElement>(null);
  const onKey = (e: KeyboardEvent<HTMLElement>) => {
    if (!["ArrowDown", "ArrowUp", "Home", "End"].includes(e.key)) return;
    const i = servers.findIndex((s) => s.name === current);
    if (i < 0) return;
    e.preventDefault();
    const next = e.key === "Home" ? 0 : e.key === "End" ? servers.length - 1 : Math.max(0, Math.min(servers.length - 1, i + (e.key === "ArrowDown" ? 1 : -1)));
    onSelect(servers[next].name);
    ref.current?.querySelectorAll<HTMLElement>("[role=option]")[next]?.focus();
  };
  return (
    <div className="srv-list">
      <Card list>
        <div ref={ref} role="listbox" aria-label="Declared servers" onKeyDown={onKey}>
          {servers.map((s) => {
            const sel = s.name === current;
            return (
              <ListRow
                key={s.name}
                role="option"
                aria-selected={sel}
                tabIndex={sel ? 0 : -1}
                selected={sel}
                onClick={() => onSelect(s.name)}
                title={humanize(s.name)}
                description={<span className={s.connected ? "srv-list__sum" : "srv-list__sum is-off"}>{s.connected ? summary(s) : "Not connected"}</span>}
                meta={transportWord(s.transport)}
              />
            );
          })}
        </div>
      </Card>
    </div>
  );
}

function LoadingPanes() {
  return (
    <div className="srv-panes" aria-busy="true" aria-label="Loading servers">
      <div className="srv-list">
        <Card list>
          {[120, 84, 96].map((w, i) => (
            <div key={i} className="srv-skel-row">
              <Skeleton width={w} height={12} />
              <Skeleton width={w + 60} height={10} />
            </div>
          ))}
        </Card>
      </div>
      <div className="srv-pane">
        <div className="srv-detail">
          <div className="srv-detail__header">
            <div className="srv-skel-head">
              <Skeleton width={180} height={24} />
              <Skeleton width={140} height={12} />
            </div>
            <Skeleton width={88} height={20} radius="var(--radius-full)" />
          </div>
          <SkeletonText lines={2} className="srv-skeleton-desc" />
          <Skeleton width={120} height={16} className="srv-skeleton-gap" />
          <Card list>
            {[0, 1, 2].map((i) => (
              <div key={i} className="srv-item">
                <div className="srv-item__text">
                  <Skeleton width={i === 2 ? "45%" : "62%"} height={14} />
                </div>
              </div>
            ))}
          </Card>
        </div>
      </div>
    </div>
  );
}
