/**
 * The room: the posture for someone across the room (docs/design-system.md, The two
 * postures). Ghost's presence fills the window; one state at display size; the few things that
 * need the operator in large type; and what Ghost is doing right now, with its live clocks.
 * Everything on it is true: the counts are today's records, the clocks are the runs'.
 *
 * In a spoken session on the conversation screen, the conversation screen's own room view is
 * the room (the shell only takes the rail away). Anywhere else the shell lays this over the desk,
 * which stays mounted underneath, so a draft or a dialog survives a trip to the room.
 */
import { useGhost } from "@ghost/ui";
import { ArrowRight, MicOff, MonitorDown } from "lucide-react";
import { useEffect, useRef, useState } from "react";

import { Button, LineGlyph, Mascot, VoiceWave, cx, formatShortcut, useLive } from "../ui";
import { notListening, notListeningSub } from "./listening";
import { usePosture } from "./PostureContext";
import { moodOf } from "./Rail";
import { hrefFor } from "./router";
import { nameOf, useToday } from "./today";
import { clock, needLine } from "../screens/today/model";

/** The room shows at most this many things that need the operator: more is the desk's job. */
const ROOM_NEEDS = 3;

export function Room() {
  const { status, turns, pending } = useGhost();
  const { connection, phase, loadingModels } = useLive();
  const { reason, hold } = usePosture();
  const today = useToday();
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);
  const back = useRef<HTMLButtonElement>(null);
  const held = reason === "held";
  // A held room is something the operator went to: focus its way out, so Enter or Escape leaves.
  useEffect(() => {
    if (held) back.current?.focus({ preventScroll: true });
  }, [held]);

  const off = connection === "online" && phase === "passive" ? notListening(status) : null;
  const inSession = !!status?.session_id;
  const onPhone = inSession && status?.session_origin === "phone";
  const who = nameOf(today.agents, status?.conversation);
  const word = stateWord({ connection, phase, off: !!off, inSession, loadingModels, pending: !!pending });
  const sub = subLine({ connection, off, inSession, onPhone, who, loadingModels });
  const reply = inSession ? [...turns].reverse().find((t) => t.role === "assistant")?.text ?? null : null;
  const needs = [...today.needs.decide, ...today.needs.answer, ...today.needs.know];
  const shown = needs.slice(0, ROOM_NEEDS);
  const c = today.counts;
  const date = new Date(now);

  return (
    <section className={cx("room-layer", `is-${connection === "online" ? phase : "offline"}`, off && "is-off", held && "is-held")} aria-label="Room" data-posture-reason={reason}>
      <header className="room-layer__top">
        <p className="room-layer__clock t-num" aria-hidden>
          <span>{date.toLocaleTimeString([], { hour: "numeric", minute: "2-digit" })}</span>
          <span className="room-layer__date">{date.toLocaleDateString("en-US", { weekday: "long", month: "long", day: "numeric" })}</span>
        </p>
        {held ? (
          <Button ref={back} variant="secondary" size="lg" leadingIcon={<MonitorDown />} onClick={() => hold("auto")}>
            Back to the desk
          </Button>
        ) : (
          <p className="room-layer__hint">Move the mouse or press a key to come back</p>
        )}
      </header>

      <div className="room-layer__middle">
        <div className="room-layer__presence">
          <span className="room-layer__glow" aria-hidden />
          <span className="room-layer__wisp" style={{ viewTransitionName: "ghost-presence" } as React.CSSProperties}>
            <Mascot size={200} mood={moodOf(connection === "online" ? phase : "offline", !!off)} idle title="" />
          </span>
          <h2 className="room-layer__word" aria-live="polite">
            {off && <MicOff aria-hidden className="room-layer__off" />}
            {word}
          </h2>
          {sub && <p className="room-layer__sub">{sub}</p>}
          {inSession && phase === "speaking" && (
            <span className="room-layer__wave" aria-hidden>
              <VoiceWave active variant="ribbon" height={40} />
            </span>
          )}
          {reply && (
            <p className="room-layer__reply" data-user-content>
              {reply}
            </p>
          )}
        </div>

        <div className="room-layer__needs">
          {pending && inSession ? (
            <div className="room-need is-decide">
              <p className="room-need__kind">Waiting for your okay</p>
              <p className="room-need__line" data-user-content>
                {pending.summary}
              </p>
              <p className="room-need__meta">Say yes to go ahead, or no to leave it.</p>
            </div>
          ) : shown.length ? (
            <>
              <p className="room-layer__label">{needs.length === 1 ? "One thing for you" : `${needs.length} things for you`}</p>
              <ol className="room-needs" role="list">
                {shown.map((i) => {
                  const kind = i.change ? "decide" : i.question ? "answer" : "know";
                  return (
                    <li key={i.key} className={cx("room-need", `is-${kind}`)}>
                      <p className="room-need__kind">{kind === "decide" ? "Decide" : kind === "answer" ? `${i.question?.asked_by ?? "Someone"} asks` : "To know"}</p>
                      <p className="room-need__line" data-user-content>
                        {needLine(i)}
                      </p>
                    </li>
                  );
                })}
              </ol>
              {needs.length > ROOM_NEEDS && (
                <a className="room-layer__more" href={hrefFor("/")} onClick={() => hold("auto")}>
                  {needs.length - ROOM_NEEDS} more on Today <ArrowRight aria-hidden />
                </a>
              )}
              <p className="room-layer__say">{inSession ? "Ask Ghost about any of these." : "Say “Ghost” and ask about any of these."}</p>
            </>
          ) : (
            <div className="room-need is-clear">
              <p className="room-need__kind">All clear</p>
              <p className="room-need__line">Nothing needs you.</p>
            </div>
          )}
        </div>
      </div>

      <footer className="room-layer__bottom">
        <div className="room-work" aria-label="Working now">
          {today.working.length === 0 && !onPhone ? (
            <p className="room-work__quiet">No agent is working right now.</p>
          ) : (
            <ul role="list">
              {onPhone && (
                <li className="room-work__item">
                  <LineGlyph slug={status?.conversation ?? "ghost"} name={who} size="md" />
                  <span className="room-work__who">{who}</span>
                  <span className="room-work__what">on your phone</span>
                </li>
              )}
              {today.working.map((r) => (
                <li key={r.id} className="room-work__item">
                  <span className="room-work__live" aria-hidden />
                  <LineGlyph slug={r.agent} name={nameOf(today.agents, r.agent)} size="md" />
                  <span className="room-work__who">{nameOf(today.agents, r.agent)}</span>
                  <span className="room-work__what">{r.name}</span>
                  <span className="room-work__meta t-num">
                    {r.tool_calls} tool {r.tool_calls === 1 ? "call" : "calls"} · {clock(r.started_at ?? r.scheduled_for, now)}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
        <p className="room-layer__day t-num">
          <Stat n={c.changes} one="change" many="changes" />
          <Stat n={c.runs} one="run" many="runs" />
          <Stat n={c.toolCalls} one="tool call" many="tool calls" />
          {today.reach.apps > 0 && <Stat n={today.reach.apps} one="app in reach" many="apps in reach" />}
        </p>
        {held && <p className="room-layer__key">{formatShortcut("mod+.")} or Escape for the desk</p>}
      </footer>
    </section>
  );
}

function Stat({ n, one, many }: { n: number; one: string; many: string }) {
  return (
    <span className="room-stat">
      <strong>{n}</strong> {n === 1 ? one : many}
    </span>
  );
}

function stateWord(s: { connection: string; phase: string; off: boolean; inSession: boolean; loadingModels: boolean; pending: boolean }): string {
  if (s.connection === "offline") return "Ghost isn’t running";
  if (s.connection !== "online") return "Reconnecting";
  if (s.off) return "Not listening";
  if (s.phase === "starting" || s.loadingModels) return "Waking up";
  if (s.inSession) {
    if (s.pending) return "Waiting for your okay";
    if (s.phase === "speaking") return "Speaking";
    if (s.phase === "thinking") return "Working";
    return "Listening";
  }
  return "Say “Ghost”";
}

function subLine(s: { connection: string; off: ReturnType<typeof notListening>; inSession: boolean; onPhone: boolean; who: string; loadingModels: boolean }): string | null {
  if (s.connection === "offline") return "Start it from the desk, or run systemctl --user start ghostd.";
  if (s.connection !== "online") return "What you see may be out of date.";
  if (s.off) return `${notListeningSub(s.off)}. Typing still works.`;
  if (s.loadingModels) return "Loading its voice, usually under a minute.";
  if (s.inSession) return s.onPhone ? `With ${s.who}, on your phone` : `With ${s.who}`;
  return "Nothing leaves this computer until you do.";
}
