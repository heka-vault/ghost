/**
 * The rail: the desk posture's one way around (docs/design-system.md, The two postures). Ghost
 * at the top, alive (Wisp in the voice phase, the way to the conversation), then Today,
 * Watching and Apps with their labels under them, then the agents themselves (each its mark
 * and name, with its live state) and + to hire one, then the room, the microphone and Settings.
 * Choosing an agent opens the conversation with it; its settings are under Settings › Agents
 * (the owner, 2026-11-12).
 *
 * Every row is an icon over a label, so the rail reads at a glance and a screen reader hears
 * the label and the count: “Today, 2 need you”.
 */
import { useGhost } from "@ghost/ui";
import { AudioWaveform, Blocks, CircleArrowUp, Eye, FlaskConical, MonitorUp, Plus, Settings2, Sun } from "lucide-react";
import { type ReactNode, Suspense, lazy, useRef, useState } from "react";

import { useDesign } from "../design/context";
import { GhostChip, IconButton, LineGlyph, Tooltip, cx, formatShortcut, liveLabel, useLive, versionLine } from "../ui";
import { type RailAgent, railAgents } from "./railAgents";
import { ListeningSwitch, useListeningShortcut } from "./ListeningSwitch";
import { notListening, notListeningSub, sessionWords, typedSession } from "./listening";
import { markStateOf, moodOf } from "./mood";
import { usePostureControl } from "./PostureContext";
import { hrefFor, navigate, usePath } from "./router";
import { navMatch } from "./routes";
import { nameOf, useToday } from "./today";
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
  { id: "watching", to: "/watching", label: "Watching", icon: <Eye />, match: ["watching"] },
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
  const posture = usePostureControl();
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
  const needs = today.needs.decide.length + today.needs.answer.length;
  const working = today.working.length + (inSession ? 1 : 0);
  const inSettings = path.startsWith("/settings");
  const agents = railAgents({ agents: today.agents, working: today.working, needs: [...today.needs.decide, ...today.needs.answer], session: inSession ? (status?.conversation ?? "ghost") : null, phase });
  const openAgent = path.startsWith("/agents/") ? decodeURIComponent(path.slice("/agents/".length)) : null;
  const hire = () => {
    // The roster's own dialog, opened from its address (?open=new, before the hash).
    const q = new URLSearchParams(window.location.search);
    q.set("open", "new");
    window.history.replaceState(window.history.state, "", `${window.location.pathname}?${q}${window.location.hash}`);
    navigate("/settings/agents");
  };
  const showSwitch = connection !== "offline" && connection !== "connecting";
  // ADR 0066: a new version, said quietly; Settings › General has the button.
  const [update] = useUpdate(DESKTOP_VERSION);
  const updateLine = availableLine(update);
  const version = useRef<{ line: string; other: string | null } | null>(null);
  if (status?.version) version.current = { line: versionLine(status.version, status.build), other: difference(status)?.window ?? null };

  const badge = (id: string): { n: number | null; label: string } | null => {
    if (id === "today" && needs) return { n: needs, label: `${needs} need${needs === 1 ? "s" : ""} you` };
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

      <nav className="rail__nav" aria-label="Main">
        <ul role="list">
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
        <div className="rail__agents" role="group" aria-label="Agents">
          <IconButton label="Hire an agent" tooltipSide="right" icon={<Plus />} onClick={hire} className="rail__hire" />
          <ul role="list">
            {agents.map((a) => (
              <li key={a.slug}>
                <AgentRow a={a} on={openAgent === a.slug} />
              </li>
            ))}
          </ul>
        </div>
      </nav>

      <div className="rail__foot">
        <ul role="list">
          <li>
            <a className={cx("rail__item rail__item--small", current === "hear" && "is-current")} href={hrefFor("/hear")} aria-current={current === "hear" ? "page" : undefined}>
              <span className="rail__icon">
                <AudioWaveform />
              </span>
              <span className="rail__label">Hearing</span>
            </a>
          </li>
          {updateLine && (
            <li>
              <Tooltip side="right" content={updateLine} asChild>
                <a className="rail__item rail__item--small rail__update" href={hrefFor("/settings/general/updates")} aria-label={updateLine}>
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
          <IconButton label={`Room (${formatShortcut("mod+.")})`} tooltipSide="right" icon={<MonitorUp />} onClick={() => posture.hold("room")} className="rail__room" />
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
