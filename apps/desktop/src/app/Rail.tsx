/**
 * The rail: the desk posture's one way around (docs/design-system.md, The two postures). Ghost
 * at the top, alive (Wisp in the voice phase, the way to the conversation), then Today, On its
 * own and Apps with their labels under them, then the agents that matter now (each its mark and
 * name, with its live state; `railAgents.ts` picks them and holds their order) and All agents
 * for the rest and for hiring, then Settings, the floating room's switch and the microphone.
 * Four places and the agents (the owner's simplification, 2026-11-14): the Hearing test is
 * reached from Settings › Sound and the microphone's notes, not from here.
 * Choosing an agent opens the conversation with it; its settings are under Settings › Agents
 * (the owner, 2026-11-12).
 *
 * Every row is an icon over a label, so the rail reads at a glance and a screen reader hears
 * the label and the count: “Today, 2 need you”.
 */
import { RailDebugMark } from "./DebugMark";
import { useGhost } from "@ghost/ui";
import { Blocks, CircleArrowUp, FlaskConical, MessagesSquare, Repeat, Settings2, Sun } from "lucide-react";
import { type ReactNode, Suspense, lazy, useLayoutEffect, useMemo, useRef, useState } from "react";

import { useDesign } from "../design/context";
import { GhostChip, LineGlyph, Tooltip, cx, liveLabel, useLive, versionLine } from "../ui";
import { RAIL_MAX, type RailAgent, fitOf, holdStill, railAgents, railShown } from "./railAgents";
import { ListeningSwitch, useListeningShortcut } from "./ListeningSwitch";
import { notListening, notListeningSub, sessionWords, typedSession } from "./listening";
import { markStateOf, moodOf } from "./mood";
import { FloatingSwitch } from "./FloatingSwitch";
import { hrefFor, usePath } from "./router";
import { navMatch } from "./routes";
import { nameOf, useToday } from "./today";
import { liveNeeds } from "../screens/today/live";
import { availableLine, useUpdate } from "./update";
import { DESKTOP_VERSION, difference } from "./version";

/** Design-mode chrome, compiled only in development. */
const DevMenu = import.meta.env.DEV ? lazy(() => import("../design/chrome").then((m) => ({ default: m.DevMenu }))) : null;

interface Item {
  id: string;
  to: string;
  label: string;
  icon: ReactNode;
  /** Route ids that light this row. */
  match: string[];
}

const MAIN: Item[] = [
  { id: "today", to: "/", label: "Today", icon: <Sun />, match: ["today"] },
  { id: "watching", to: "/on-its-own", label: "On its own", icon: <Repeat />, match: ["watching"] },
  { id: "apps", to: "/apps", label: "Apps", icon: <Blocks />, match: ["apps"] },
];

/** Wisp's pose for a voice phase (the mascot's own vocabulary). */

export function Rail() {
  const path = usePath();
  const current = navMatch(path);
  const design = useDesign();
  const { status } = useGhost();
  const { connection, phase } = useLive();
  const today = useToday();
  useListeningShortcut();
  const [hover, setHover] = useState(false);

  const off = connection === "online" && phase === "passive" ? notListening(status) : null;
  const live = liveLabel(connection, phase);
  const inSession = !!status?.session_id;
  // A typed session opened no microphone (2026-11-13): it says typing, never listening.
  const typed = typedSession(status);
  const state = off ? "Not listening" : connection !== "online" ? live.title : inSession ? (phase === "speaking" ? "Speaking" : phase === "thinking" ? "Working" : typed ? "Typing" : "Listening") : phase === "starting" ? "Starting" : "Ready";
  const sub = off
    ? notListeningSub(off)
    : connection === "offline"
      ? "The Ghost service is stopped"
      : connection === "reconnecting"
        ? "Trying again now"
        : inSession
          ? status?.session_origin === "phone"
            ? "On your phone"
            : (sessionWords(status, status?.conversation && status.conversation !== "ghost" ? nameOf(today.agents, status.conversation) : "Ghost") ?? "With Ghost")
          : phase === "starting"
            ? "Loading voice models"
            : "Listening for its name";
  // The same count as Today's headline: the live session's change once, even before the list has it.
  const open = liveNeeds(today.needs, inSession ? (status?.pending ?? null) : null);
  const needs = open.decide.length + open.answer.length;
  const working = today.working.length + (inSession ? 1 : 0);
  const inRoster = path === "/agents" || /^\/agents\/[^/]+\/settings/.test(path);
  const inSettings = path.startsWith("/settings");
  const all = railAgents({ agents: today.agents, working: today.working, needs: [...open.decide, ...open.answer], session: inSession ? (status?.conversation ?? "ghost") : null, phase });
  const openAgent = path.startsWith("/agents/") ? decodeURIComponent(path.slice("/agents/".length).split("/")[0]) : null;
  // The room the agents have, measured, and their order held while the pointer is on them.
  const { navRef, mainRef, groupRef, listRef, fit } = useRailFit();
  const [hold, setHold] = useState(false);
  const { shown: picked, hidden } = railShown(all, fit);
  const before = useRef<string[]>([]);
  const order = useMemo(() => holdStill(before.current, picked.map((a) => a.slug), hold), [picked.map((a) => a.slug).join(","), hold]); // eslint-disable-line react-hooks/exhaustive-deps
  before.current = order;
  const bySlug = new Map(all.map((a) => [a.slug, a]));
  const agents = order.map((s) => bySlug.get(s)).filter((a): a is RailAgent => !!a);
  const allWords = all.length === 0 ? "none hired yet" : hidden > 0 ? `${hidden} more` : `${all.length} in all`;
  const showSwitch = connection !== "offline" && connection !== "connecting";
  // ADR 0066: a new version, said quietly; Settings › General has the button.
  const [update] = useUpdate(DESKTOP_VERSION);
  const updateLine = availableLine(update);
  const version = useRef<{ line: string; other: string | null } | null>(null);
  if (status?.version) version.current = { line: versionLine(status.version, status.build), other: difference(status)?.window ?? null };

  const running = today.working.filter((r) => r.status === "running").length;
  const badge = (id: string): { n: number | null; label: string } | null => {
    if (id === "today" && needs) return { n: needs, label: `${needs} need${needs === 1 ? "s" : ""} you` };
    // Running now stays loud: a pulse on the place where it can be stopped.
    if (id === "watching" && running) return { n: null, label: `${running} running now` };
    return null;
  };

  return (
    <aside className="rail" aria-label="Ghost">
      <Tooltip
        side="right"
        content={
          <span className="rail__tip">
            <strong>{state}</strong>
            {sub && <span>{sub}</span>}
            {version.current && <span className="rail__tip-meta">{version.current.line}</span>}
            {version.current?.other && <span className="rail__tip-meta">This window is {version.current.other}</span>}
          </span>
        }
        asChild
      >
        <a
          className={cx("rail__presence", `is-${live.dot}`, off && "is-off", current === "session" && "is-current")}
          href={hrefFor("/session")}
          aria-current={current === "session" ? "page" : undefined}
          aria-label={`Conversation. Ghost: ${state}${sub ? `, ${sub}` : ""}${working > 1 ? `. ${working} at work` : ""}`}
          onPointerEnter={(e) => e.pointerType === "mouse" && setHover(true)}
          onPointerLeave={() => setHover(false)}
        >
          <span className="rail__wisp">
            <GhostChip size={44} ring="arc" state={markStateOf(phase, !!off, !!status?.pending)} mood={moodOf(phase, !!off, typed)} hovered={hover} />
          </span>
          <span className="rail__state">{state}</span>
        </a>
      </Tooltip>
      <RailDebugMark status={status} />

      <nav className="rail__nav" aria-label="Main" ref={navRef}>
        <ul role="list" ref={mainRef}>
          {MAIN.map((it) => {
            const on = !!current && it.match.includes(current);
            const b = badge(it.id);
            return (
              <li key={it.id}>
                <a className={cx("rail__item", on && "is-current")} href={hrefFor(it.to)} aria-current={on ? "page" : undefined} aria-label={b ? `${it.label}, ${b.label}` : undefined}>
                  <span className="rail__icon">
                    {it.icon}
                    {b && (b.n != null ? <span className="rail__count t-num">{b.n}</span> : <span className="rail__pulse" aria-hidden />)}
                  </span>
                  <span className="rail__label">{it.label}</span>
                </a>
              </li>
            );
          })}
        </ul>
        <div
          className="rail__agents"
          role="group"
          aria-label="Agents"
          ref={groupRef}
          onPointerEnter={() => setHold(true)}
          onPointerLeave={() => setHold(false)}
          onFocus={() => setHold(true)}
          onBlur={(e) => !e.currentTarget.contains(e.relatedTarget as Node | null) && setHold(false)}
        >
          <ul role="list" ref={listRef}>
            {agents.map((a) => (
              <li key={a.slug}>
                <AgentRow a={a} on={openAgent === a.slug} />
              </li>
            ))}
          </ul>
          <Tooltip side="right" content={`Every agent, and hiring one · ${allWords}`} asChild>
            <a
              className={cx("rail__agent rail__all", inRoster && "is-current")}
              href={hrefFor("/agents")}
              aria-current={inRoster ? "page" : undefined}
              aria-label={`All agents, ${allWords}`}
            >
              <span className="rail__agent-mark rail__all-mark">
                <MessagesSquare />
                {hidden > 0 && <span className="rail__more t-num" aria-hidden>+{hidden}</span>}
              </span>
              <span className="rail__label rail__agent-name">All agents</span>
            </a>
          </Tooltip>
        </div>
      </nav>

      <div className="rail__foot">
        <ul role="list">
          {updateLine && (
            <li>
              <Tooltip side="right" content={updateLine} asChild>
                <a className="rail__item rail__item--small rail__update" href={hrefFor("/settings/about/updates")} aria-label={updateLine}>
                  <span className="rail__icon">
                    <CircleArrowUp />
                  </span>
                  <span className="rail__label">Update</span>
                </a>
              </Tooltip>
            </li>
          )}
          <li>
            <a className={cx("rail__item rail__item--small", inSettings && "is-current")} href={hrefFor("/settings/general")} aria-current={inSettings ? "page" : undefined}>
              <span className="rail__icon">
                <Settings2 />
              </span>
              <span className="rail__label">Settings</span>
            </a>
          </li>
        </ul>
        <div className="rail__controls">
          <FloatingSwitch />
          {showSwitch && <ListeningSwitch rail />}
        </div>
        {design && DevMenu && (
          <Suspense fallback={<span className="rail__dev" aria-hidden><FlaskConical /></span>}>
            <DevMenu />
          </Suspense>
        )}
      </div>
    </aside>
  );
}

/**
 * How many agent rows fit in the rail's nav without scrolling, measured from the layout and again
 * whenever the nav changes size (the window, the names going under 760px, an update row at the foot).
 */
function useRailFit() {
  const navRef = useRef<HTMLElement>(null);
  const mainRef = useRef<HTMLUListElement>(null);
  const groupRef = useRef<HTMLDivElement>(null);
  const listRef = useRef<HTMLUListElement>(null);
  const [fit, setFit] = useState(RAIL_MAX);
  useLayoutEffect(() => {
    const nav = navRef.current;
    if (!nav || typeof ResizeObserver === "undefined") return;
    const measure = () => {
      // All agents is a row of the same shape: one row's height (the Tooltip holds its ref).
      const main = mainRef.current, group = groupRef.current, list = listRef.current, one = group?.querySelector<HTMLElement>(".rail__all");
      if (!main || !group || !list || !one) return;
      const css = getComputedStyle(nav);
      const gap = parseFloat(getComputedStyle(list).rowGap) || 0;
      // Everything the nav holds but the agents' own rows: the places, the group's edge and
      // margin, All agents, the nav's padding (its fade at the foot included).
      const rest = parseFloat(css.paddingTop) + parseFloat(css.paddingBottom) + main.offsetHeight + parseFloat(getComputedStyle(group).marginTop) + group.offsetHeight - list.offsetHeight;
      setFit(fitOf({ navHeight: nav.clientHeight + gap, restHeight: rest, rowPitch: one.offsetHeight + gap }));
    };
    const ro = new ResizeObserver(measure);
    ro.observe(nav);
    measure();
    return () => ro.disconnect();
  }, []);
  return { navRef, mainRef, groupRef, listRef, fit };
}

/** One agent on the rail: its mark with its live state, its name under it; opens its conversation. */
function AgentRow({ a, on }: { a: RailAgent; on: boolean }) {
  return (
    <Tooltip side="right" content={`${a.name}${a.stateWords ? ` · ${a.stateWords}` : ""}`} asChild>
      <a
        className={cx("rail__agent", on && "is-current", a.state && `is-${a.state}`)}
        href={hrefFor(`/agents/${encodeURIComponent(a.slug)}`)}
        aria-current={on ? "page" : undefined}
        aria-label={`${a.name}${a.stateWords ? `, ${a.stateWords}` : ""}`}
      >
        <span className="rail__agent-mark">
          <LineGlyph slug={a.slug} name={a.name} size="lg" />
          {a.waiting > 0 ? <span className="rail__count t-num">{a.waiting}</span> : a.state === "working" ? <span className="rail__pulse" aria-hidden /> : null}
        </span>
        <span className="rail__label rail__agent-name">{a.name}</span>
      </a>
    </Tooltip>
  );
}
