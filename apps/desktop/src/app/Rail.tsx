/**
 * The rail: the desk posture's one way around (docs/design-system.md, The two postures). Ghost
 * at the top, alive (Wisp in the voice phase, the way to the conversation), then Today,
 * Agents, Watching and Apps with their labels under them, then the room, the microphone and
 * Settings. No agent list: agents live on Agents and in the conversation (overhaul, step 1).
 *
 * Every row is an icon over a label, so the rail reads at a glance and a screen reader hears
 * the label and the count: “Today, 2 need you”.
 */
import { useGhost } from "@ghost/ui";
import { AudioWaveform, Blocks, Eye, FlaskConical, MonitorUp, Settings2, Sun, Users } from "lucide-react";
import { type ReactNode, Suspense, lazy, useRef, useState } from "react";

import { useDesign } from "../design/context";
import { type GhostPhase, IconButton, Mascot, Tooltip, cx, formatShortcut, liveLabel, useLive, versionLine } from "../ui";
import { ListeningSwitch, useListeningShortcut } from "./ListeningSwitch";
import { notListening, notListeningSub } from "./listening";
import { usePostureControl } from "./PostureContext";
import { hrefFor, usePath } from "./router";
import { navMatch } from "./routes";
import { nameOf, useToday } from "./today";
import { difference } from "./version";

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
  { id: "agents", to: "/agents", label: "Agents", icon: <Users />, match: ["conversations"] },
  { id: "watching", to: "/watching", label: "Watching", icon: <Eye />, match: ["watching"] },
  { id: "apps", to: "/settings/apps", label: "Apps", icon: <Blocks />, match: ["apps"] },
];

/** Wisp's pose for a voice phase (the mascot's own vocabulary). */
export function moodOf(phase: GhostPhase, off: boolean): "listening" | "thinking" | "speaking" | "starting" | "offline" | "passive" | "hold" {
  if (off) return "hold";
  if (phase === "listening" || phase === "thinking" || phase === "speaking" || phase === "starting" || phase === "offline") return phase;
  return "passive";
}

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
  const state = off ? "Not listening" : connection !== "online" ? live.title : inSession ? (phase === "speaking" ? "Speaking" : phase === "thinking" ? "Working" : "Listening") : phase === "starting" ? "Starting" : "Ready";
  const sub = off
    ? notListeningSub(off)
    : connection === "offline"
      ? "The Ghost service is stopped"
      : connection === "reconnecting"
        ? "Trying again now"
        : inSession
          ? status?.session_origin === "phone"
            ? "On your phone"
            : `With ${status?.conversation && status.conversation !== "ghost" ? nameOf(today.agents, status.conversation) : "Ghost"}`
          : phase === "starting"
            ? "Loading voice models"
            : "Listening for its name";
  const needs = today.needs.decide.length + today.needs.answer.length;
  const working = today.working.length + (inSession ? 1 : 0);
  const inSettings = path.startsWith("/settings") && current !== "apps";
  const showSwitch = connection !== "offline" && connection !== "connecting";
  const version = useRef<{ line: string; other: string | null } | null>(null);
  if (status?.version) version.current = { line: versionLine(status.version, status.build), other: difference(status)?.window ?? null };

  const badge = (id: string): { n: number | null; label: string } | null => {
    if (id === "today" && needs) return { n: needs, label: `${needs} need${needs === 1 ? "s" : ""} you` };
    if (id === "agents" && today.working.length) return { n: null, label: `${today.working.length} working now` };
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
          style={{ viewTransitionName: "ghost-presence" } as React.CSSProperties}
        >
          <span className="rail__wisp">
            <Mascot size={36} mood={moodOf(phase, !!off)} hovered={hover} title="" />
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
