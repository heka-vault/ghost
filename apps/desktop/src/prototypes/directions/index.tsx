/**
 * Owner: design. Three directions for the whole app (docs/design/overhaul-2026-11.md), as
 * clickable pages over the mock daemon's live data, so the owner can open them and choose:
 *
 *   room       The room: the conversation is the app. No sidebar; what needs you is what
 *              Ghost will bring up, said the way Ghost would say it; agents are people you call.
 *   briefing   The briefing: a chief of staff's desk. Home is today: what needs you, grouped by
 *              what you do about it, beside what happened; talking to Ghost is a bar.
 *   console    The console: everything at once. Queue and conversation side by side, agents
 *              as a rail; dense, for the desk.
 *
 * Each shows Home and Agents. `#/directions/<name>/<home|agents>`, with `?mock=busy` for data.
 * Not a screen: nothing here is restyled for real until the owner picks.
 */
import type { AuditEntry, ConversationView, WaitingItemView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { ArrowRight, Bell, Eye, Inbox, LayoutGrid, MessagesSquare, Mic, Phone, Settings, Sparkles, Users } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import { Link } from "../../app/router";
import type { RouteProps } from "../../app/routes";
import { Button, type GhostPhase, LineGlyph, VoicePresence, VoiceWave, cx, relativeTime, useLive } from "../../ui";
import "./directions.css";

type Name = "room" | "briefing" | "console";
type Screen = "home" | "agents";
const NAMES: { name: Name; label: string; trade: string }[] = [
  { name: "room", label: "The room", trade: "Most voice first; least to scan at the desk." },
  { name: "briefing", label: "The briefing", trade: "Best at the desk; voice is one bar." },
  { name: "console", label: "The console", trade: "Everything at once; busy, worst from across a room." },
];

interface Data {
  waiting: WaitingItemView[];
  agents: ConversationView[];
  changes: AuditEntry[];
}

function useData(): Data {
  const { client, waitingVersion, conversationsVersion } = useGhost();
  const [data, setData] = useState<Data>({ waiting: [], agents: [], changes: [] });
  useEffect(() => {
    let live = true;
    Promise.all([client.waiting().catch(() => null), client.conversations().catch(() => null), client.audit({ limit: 8 }).catch(() => null)]).then(([w, a, au]) => {
      if (!live) return;
      setData({
        waiting: (w?.items ?? []).filter((i) => i.state !== "acknowledged"),
        agents: (a ?? []).filter((x) => !x.retired_at),
        changes: au?.items ?? [],
      });
    });
    return () => {
      live = false;
    };
  }, [client, waitingVersion, conversationsVersion]);
  return data;
}

export default function Directions({ params }: RouteProps) {
  const name = (NAMES.some((n) => n.name === params.name) ? params.name : "briefing") as Name;
  const screen: Screen = params.screen === "agents" ? "agents" : "home";
  const data = useData();
  return (
    <div className={cx("dir", `dir--${name}`)}>
      {name === "room" && <Room screen={screen} data={data} />}
      {name === "briefing" && <Briefing screen={screen} data={data} />}
      {name === "console" && <Console screen={screen} data={data} />}
      <Switcher name={name} screen={screen} />
    </div>
  );
}

/** Which direction and which screen: a small floating bar, for the comparison only. */
function Switcher({ name, screen }: { name: Name; screen: Screen }) {
  const current = NAMES.find((n) => n.name === name)!;
  return (
    <nav className="dir-switch" aria-label="Directions">
      {NAMES.map((n) => (
        <Link key={n.name} to={`/directions/${n.name}/${screen}`} className={cx("dir-switch__item", n.name === name && "is-on")}>
          {n.label}
        </Link>
      ))}
      <span className="dir-switch__sep" />
      {(["home", "agents"] as Screen[]).map((s) => (
        <Link key={s} to={`/directions/${name}/${s}`} className={cx("dir-switch__item", s === screen && "is-on")}>
          {s === "home" ? "Home" : "Agents"}
        </Link>
      ))}
      <span className="dir-switch__trade">{current.trade}</span>
    </nav>
  );
}

// ---------------------------------------------------------------- shared bits

type Kind = "answer" | "decide" | "know";

/** A waiting item and the one thing to do about it. */
function said(i: WaitingItemView): { line: string; action: string; kind: Kind } {
  if (i.question) return { line: i.title, action: "Answer", kind: "answer" };
  if (i.change) return { line: i.title.replace(/^Waiting for your yes: /, ""), action: "Decide", kind: "decide" };
  return { line: i.title, action: "Got it", kind: "know" };
}

function from(source: string): string {
  return source.replace(/^agent:/, "").replace(/^client:/, "").replace(/^watch:/, "watch ");
}

function useStateWords(): { phase: GhostPhase; title: string } {
  const { phase, online } = useLive();
  const { status } = useGhost();
  const who = status?.conversation && status.conversation !== "ghost" ? status.conversation : null;
  const title = !online ? "Reconnecting" : phase === "passive" ? "Say “Hey Ghost”" : phase === "speaking" ? "Speaking" : phase === "thinking" ? "Working" : `Listening${who ? ` · with ${who}` : ""}`;
  return { phase, title };
}

const time = (iso: string) => new Date(iso).toLocaleTimeString([], { hour: "numeric", minute: "2-digit" });

// ---------------------------------------------------------------- A. the room

function Room({ screen, data }: { screen: Screen; data: Data }) {
  const { phase, title } = useStateWords();
  return (
    <div className="room-dir">
      <header className="room-dir__bar">
        <span className="room-dir__brand">
          <LineGlyph slug="ghost" name="Ghost" size="lg" /> Ghost
        </span>
        <nav className="room-dir__tabs">
          <Link to="/directions/room/home" className={cx(screen === "home" && "is-on")}>
            Room
          </Link>
          <Link to="/directions/room/agents" className={cx(screen === "agents" && "is-on")}>
            Agents
          </Link>
          <span>Watching</span>
          <span>Settings</span>
        </nav>
      </header>
      {screen === "home" ? (
        <main className="room-dir__home">
          <div className="room-dir__presence">
            <VoicePresence phase={phase} size={160} label="" />
            <h1 className="room-dir__title">{title}</h1>
            <p className="room-dir__sub">Ghost will bring these up when you talk.</p>
          </div>
          <ol className="room-dir__queue">
            {data.waiting.slice(0, 4).map((i) => {
              const s = said(i);
              return (
                <li key={i.key} className={cx("room-dir__card", `is-${s.kind}`)}>
                  <p className="room-dir__card-line">{s.line}</p>
                  <p className="room-dir__card-meta">
                    {from(i.source)} · {relativeTime(i.first_seen)}
                  </p>
                  <Button size="sm" variant={s.kind === "know" ? "ghost" : "secondary"}>
                    {s.action}
                  </Button>
                </li>
              );
            })}
          </ol>
        </main>
      ) : (
        <main className="room-dir__agents">
          <h1 className="room-dir__h">Who you can talk to</h1>
          <div className="room-dir__grid">
            {data.agents.map((a) => (
              <article key={a.slug} className={cx("room-dir__agent", a.active && "is-live")}>
                <LineGlyph slug={a.slug} name={a.name} size="xl" />
                <h2>{a.name}</h2>
                <p>{a.job || "General"}</p>
                <p className="room-dir__agent-when">{a.active ? "On the line now" : a.last_active_at ? `Last spoke ${relativeTime(a.last_active_at)}` : "Not spoken to yet"}</p>
                <Button size="sm" variant={a.active ? "primary" : "secondary"} leadingIcon={<Phone />}>
                  {a.active ? "Talking" : `Call ${a.name}`}
                </Button>
              </article>
            ))}
          </div>
        </main>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- B. the briefing

const GROUPS: { kind: Kind; title: string }[] = [
  { kind: "decide", title: "Decide" },
  { kind: "answer", title: "Answer" },
  { kind: "know", title: "Know" },
];

function Briefing({ screen, data }: { screen: Screen; data: Data }) {
  const { phase, title } = useStateWords();
  const today = new Date().toLocaleDateString(undefined, { weekday: "long", month: "long", day: "numeric" });
  const count = (k: Kind) => data.waiting.filter((i) => said(i).kind === k).length;
  const needs = count("decide") + count("answer");
  return (
    <div className="brief">
      <aside className="brief__rail">
        <span className="brief__brand">
          <LineGlyph slug="ghost" name="Ghost" size="lg" />
        </span>
        <RailLink to="/directions/briefing/home" on={screen === "home"} icon={<Sparkles />} label="Today" />
        <RailLink to="/directions/briefing/agents" on={screen === "agents"} icon={<Users />} label="Agents" />
        <RailLink icon={<Eye />} label="Watching" />
        <RailLink icon={<LayoutGrid />} label="Apps" />
        <span className="brief__rail-gap" />
        <RailLink icon={<Settings />} label="Settings" />
      </aside>
      <main className="brief__main">
        {screen === "home" ? (
          <>
            <header className="brief__head">
              <p className="brief__date">{today}</p>
              <h1 className="brief__h1">
                {needs > 0 ? `${needs} ${needs === 1 ? "thing needs" : "things need"} you` : "Nothing needs you"}
                <span className="brief__h1-soft">
                  {" "}
                  · {data.changes.length} changes made · {count("know")} to know
                </span>
              </h1>
            </header>
            <div className="brief__cols">
              <section className="brief__needs">
                {GROUPS.map((g) => {
                  const items = data.waiting.filter((i) => said(i).kind === g.kind);
                  if (!items.length) return null;
                  return (
                    <div key={g.kind} className="brief__group">
                      <h2 className="brief__group-h">
                        {g.title} <span>{items.length}</span>
                      </h2>
                      {items.map((i) => (
                        <div key={i.key} className="brief__item">
                          <span className={cx("brief__dot", `is-${g.kind}`)} />
                          <div className="brief__item-body">
                            <p className="brief__item-line">{said(i).line}</p>
                            <p className="brief__item-meta">
                              {from(i.source)} · {relativeTime(i.first_seen)}
                            </p>
                          </div>
                          <Button size="sm" variant={g.kind === "decide" ? "primary" : "secondary"}>
                            {said(i).action}
                          </Button>
                        </div>
                      ))}
                    </div>
                  );
                })}
              </section>
              <section className="brief__log">
                <h2 className="brief__group-h">What happened</h2>
                <ol className="brief__timeline">
                  {data.changes.map((c) => (
                    <li key={c.id}>
                      <time>{time(c.created_at)}</time>
                      <span className={cx("brief__tick", c.status === "executed" ? "is-ok" : c.status === "failed" ? "is-bad" : "")} />
                      <p>{c.summary}</p>
                    </li>
                  ))}
                </ol>
              </section>
            </div>
          </>
        ) : (
          <>
            <header className="brief__head">
              <h1 className="brief__h1">Agents</h1>
            </header>
            <table className="brief__table">
              <thead>
                <tr>
                  <th>Agent</th>
                  <th>Job</th>
                  <th>Last spoke</th>
                  <th aria-label="Open" />
                </tr>
              </thead>
              <tbody>
                {data.agents.map((a) => (
                  <tr key={a.slug}>
                    <td>
                      <span className="brief__agent">
                        <LineGlyph slug={a.slug} name={a.name} size="md" /> {a.name}
                        {a.active && <span className="brief__live">Live</span>}
                      </span>
                    </td>
                    <td className="brief__soft">{a.job || "General"}</td>
                    <td className="brief__soft">{a.last_active_at ? relativeTime(a.last_active_at) : "never"}</td>
                    <td>
                      <Button size="sm" variant="ghost" trailingIcon={<ArrowRight />}>
                        Open
                      </Button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </>
        )}
        <div className="brief__call">
          <VoicePresence phase={phase} size={48} label="" />
          <span className="brief__call-title">{title}</span>
          <span className="brief__call-wave">
            <VoiceWave active={phase === "speaking"} variant="ribbon" height={24} />
          </span>
          <Button size="sm" variant="secondary" leadingIcon={<Mic />}>
            Talk
          </Button>
        </div>
      </main>
    </div>
  );
}

function RailLink({ to, on, icon, label }: { to?: string; on?: boolean; icon: ReactNode; label: string }) {
  const inner = (
    <>
      {icon}
      <span>{label}</span>
    </>
  );
  return to ? (
    <Link to={to} className={cx("brief__rail-link", on && "is-on")}>
      {inner}
    </Link>
  ) : (
    <span className="brief__rail-link">{inner}</span>
  );
}

// ---------------------------------------------------------------- C. the console

function Console({ screen, data }: { screen: Screen; data: Data }) {
  const { turns } = useGhost();
  const { phase, title } = useStateWords();
  const [picked, setPicked] = useState<string | null>(null);
  const agent = data.agents.find((a) => a.slug === picked) ?? data.agents[0];
  return (
    <div className="con">
      <aside className="con__rail">
        <Link to="/directions/console/home" className={cx("con__rail-btn", screen === "home" && "is-on")} aria-label="Queue">
          <Inbox />
        </Link>
        <Link to="/directions/console/agents" className={cx("con__rail-btn", screen === "agents" && "is-on")} aria-label="Agents">
          <MessagesSquare />
        </Link>
        <span className="con__rail-btn">
          <Eye />
        </span>
        <span className="con__rail-btn">
          <Bell />
        </span>
        <span className="con__rail-sep" />
        {data.agents.slice(0, 8).map((a) => (
          <span key={a.slug} className={cx("con__rail-agent", a.active && "is-live")} title={a.name}>
            <LineGlyph slug={a.slug} name={a.name} size="lg" />
          </span>
        ))}
      </aside>
      <section className="con__list">
        <h1 className="con__h">{screen === "home" ? `Queue · ${data.waiting.length}` : `Agents · ${data.agents.length}`}</h1>
        {screen === "home"
          ? data.waiting.map((i) => (
              <div key={i.key} className="con__row">
                <span className={cx("brief__dot", `is-${said(i).kind}`)} />
                <div>
                  <p className="con__row-title">{said(i).line}</p>
                  <p className="con__row-meta">
                    {from(i.source)} · {relativeTime(i.first_seen)}
                  </p>
                </div>
              </div>
            ))
          : data.agents.map((a) => (
              <button key={a.slug} type="button" className={cx("con__row con__row--btn", agent?.slug === a.slug && "is-on")} onClick={() => setPicked(a.slug)}>
                <LineGlyph slug={a.slug} name={a.name} size="md" />
                <div>
                  <p className="con__row-title">{a.name}</p>
                  <p className="con__row-meta">{a.job || "General"}</p>
                </div>
              </button>
            ))}
      </section>
      <section className="con__pane">
        {screen === "home" || !agent ? (
          <>
            <header className="con__pane-head">
              <VoicePresence phase={phase} size={48} label="" />
              <span className="con__pane-title">{title}</span>
            </header>
            <div className="con__talk">
              {turns.slice(-8).map((t, i) => (
                <p key={i} className={cx("con__turn", t.role === "user" && "is-you")}>
                  {t.text}
                </p>
              ))}
              {!turns.length && <p className="con__empty">No session open. Say “Hey Ghost”, or type.</p>}
            </div>
            <div className="con__composer">Type to Ghost…</div>
          </>
        ) : (
          <>
            <header className="con__pane-head">
              <LineGlyph slug={agent.slug} name={agent.name} size="xl" />
              <div className="con__pane-who">
                <span className="con__pane-title">{agent.name}</span>
                <p className="con__row-meta">{agent.job || "General"}</p>
              </div>
              <Button size="sm" variant="primary" leadingIcon={<Phone />}>
                Call
              </Button>
            </header>
            <dl className="con__facts">
              <dt>Last spoke</dt>
              <dd>{agent.last_active_at ? relativeTime(agent.last_active_at) : "never"}</dd>
              <dt>Voice</dt>
              <dd>{agent.voice}</dd>
              <dt>Aliases</dt>
              <dd>{agent.aliases.join(", ") || "none"}</dd>
            </dl>
          </>
        )}
      </section>
    </div>
  );
}
