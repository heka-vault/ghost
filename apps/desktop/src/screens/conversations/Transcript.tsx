/**
 * Owner: conversations. A line's transcript as it is stored. Turns since the last
 * compaction read as a chat: bubbles, with tool calls and transfers as quiet one-liners.
 * Turns already folded into memory sit above a marker, collapsed by default; opened, they
 * read as a compact, labeled log (OpenAI Platform's transcript), so it's clear what the
 * line remembers word for word and what it only remembers as a summary.
 */
import type { TurnView } from "@ghost/client-ts";
import { ArrowDown, ArrowLeftRight, ChevronDown, ChevronUp, MessagesSquare, Wrench } from "lucide-react";
import { Fragment, useEffect, useId, useRef, useState } from "react";

import { Button, EmptyState, cx, formatTime, quoted, scrollIntoPanel } from "../../ui";

function dayLabel(iso: string): string {
  const d = new Date(iso);
  const today = new Date();
  const y = new Date(today);
  y.setDate(today.getDate() - 1);
  if (d.toDateString() === today.toDateString()) return "Today";
  if (d.toDateString() === y.toDateString()) return "Yesterday";
  return d.toLocaleDateString(undefined, { weekday: "long", month: "short", day: "numeric" });
}

export interface TranscriptProps {
  turns: TurnView[];
  lineName: string;
  /** The number asked for; when it comes back full there is older history not shown. */
  limit: number;
  /** Every turn the line has (the detail's turn_count), for the cap note. */
  total: number;
}

const isLong = (turns: TurnView[], limit: number) => turns.length >= limit;

export function Transcript({ turns, lineName, limit, total }: TranscriptProps) {
  const endRef = useRef<HTMLDivElement>(null);
  const opened = useRef(false);
  const [showFolded, setShowFolded] = useState(false);
  const logId = useId();

  // A transcript reads like a chat: it opens on what was said last, and stays there as the
  // live line adds turns, unless the operator has scrolled up to read.
  useEffect(() => {
    const end = endRef.current;
    if (!end || turns.length === 0) return;
    const scroller = end.closest<HTMLElement>(".shell__main");
    const below = scroller ? end.getBoundingClientRect().top - scroller.getBoundingClientRect().bottom : 0;
    const first = !opened.current;
    if (below > 0 && (first || below < 240)) {
      const id = requestAnimationFrame(() => {
        opened.current = true;
        scrollIntoPanel(end, { block: "panel-end", smooth: !first });
      });
      return () => cancelAnimationFrame(id);
    }
    opened.current = true;
  }, [turns.length]);

  if (turns.length === 0) {
    return (
      <EmptyState
        icon={<MessagesSquare />}
        title="Nothing said on this line yet"
        description={
          <>
            {lineName === "Ghost" ? <>Say {quoted("Hey Ghost")}.</> : <>Wake Ghost and say {quoted(`patch me to ${lineName.toLowerCase()}`)}.</>} What you both say is kept here.
          </>
        }
      />
    );
  }
  const folded = turns.filter((t) => t.compacted);
  const fresh = turns.filter((t) => !t.compacted);
  const capped = isLong(turns, limit);
  const n = folded.length;
  const turnWord = n === 1 ? "turn" : "turns";
  return (
    <div className="conv-transcript">
      {capped && (
        <div className="conv-transcript__note">
          <p className="t-small t-2">
            Showing the last {turns.length} of {Math.max(total, turns.length)} turns. Everything older is in this line’s memory.
          </p>
          {/* Only while the summarized log is open: closed, the latest is already in view. */}
          {showFolded && (
            <Button size="sm" variant="ghost" trailingIcon={<ArrowDown />} onClick={() => scrollIntoPanel(endRef.current, { block: "panel-end" })}>
              Jump to latest
            </Button>
          )}
        </div>
      )}

      {n > 0 && (
        <section className="conv-archive" aria-label="In memory">
          {/* One row of chrome: what is summarized, and the toggle that opens it, inline. */}
          <p className="conv-marker">
            <span>
              In memory · {n === turns.length ? `all ${n}` : n} {turnWord} summarized · {dayLabel(folded[0].at)}
            </span>
            <Button
              size="sm"
              variant="ghost"
              className="conv-marker__toggle"
              aria-expanded={showFolded}
              aria-controls={logId}
              aria-label={showFolded ? `Hide the ${n} summarized ${turnWord}` : `Show the ${n} summarized ${turnWord}`}
              trailingIcon={showFolded ? <ChevronUp /> : <ChevronDown />}
              onClick={() => setShowFolded(!showFolded)}
            >
              {showFolded ? "Hide" : "Show"}
            </Button>
          </p>
          <ol id={logId} className="conv-log" aria-label={`Summarized turns of ${lineName}`} hidden={!showFolded}>
            {showFolded &&
              folded.map((t, i) => {
                const prev = folded[i - 1];
                const newDay = prev && new Date(prev.at).toDateString() !== new Date(t.at).toDateString();
                return (
                  <Fragment key={t.id}>
                    {newDay && (
                      <li className="conv-log__day">
                        <span>{dayLabel(t.at)}</span>
                      </li>
                    )}
                    <LogRow turn={t} lineName={lineName} />
                  </Fragment>
                );
              })}
          </ol>
        </section>
      )}

      {fresh.length > 0 ? (
        <section aria-label={n > 0 ? "Since the last compaction" : undefined} className="conv-fresh">
          <ol className="conv-turns" aria-label={`Transcript of ${lineName}`}>
            {fresh.map((t, i) => {
              const prev = fresh[i - 1];
              const newDay = !prev || new Date(prev.at).toDateString() !== new Date(t.at).toDateString();
              return (
                <Fragment key={t.id}>
                  {newDay && (
                    <li className="conv-day">
                      <span>{dayLabel(t.at)}</span>
                    </li>
                  )}
                  <Turn turn={t} lineName={lineName} showTime={endsCluster(t, fresh[i + 1])} />
                </Fragment>
              );
            })}
          </ol>
        </section>
      ) : (
        <p className="conv-fresh-empty t-small t-3">Nothing said since the last compaction. New turns show up here.</p>
      )}
      <div ref={endRef} className="conv-transcript__end" aria-hidden />
    </div>
  );
}

/** Times show once per run of turns (bubbles and notes alike): at the end of a run, or before a pause of two minutes. */
function endsCluster(t: TurnView, next: TurnView | undefined): boolean {
  if (!next) return true;
  return new Date(next.at).getTime() - new Date(t.at).getTime() > 120_000;
}

/** One summarized turn: who, then what, left-aligned. Calls and transfers keep their icon. */
function LogRow({ turn: t, lineName }: { turn: TurnView; lineName: string }) {
  const time = formatTime(t.at);
  const note = t.role === "call" || t.role === "system";
  const Icon = t.role === "call" ? Wrench : ArrowLeftRight;
  return (
    <li className={cx("conv-log__row", note && "is-note")}>
      <span className="conv-log__who">
        {note ? <Icon size={12} aria-hidden /> : t.role === "user" ? "You" : lineName}
        <span className="sr-only">{note ? (t.role === "call" ? "Tool call" : "Transfer") : ""}: </span>
      </span>
      <span className={cx("conv-log__text", t.role === "call" && "is-call")} data-user-content>
        {t.role === "system" ? t.text[0].toUpperCase() + t.text.slice(1) : t.text}
      </span>
      <span className="conv-log__time t-num">{time}</span>
    </li>
  );
}

function Turn({ turn: t, lineName, showTime }: { turn: TurnView; lineName: string; showTime: boolean }) {
  const time = formatTime(t.at);
  if (t.role === "call" || t.role === "system") {
    const Icon = t.role === "call" ? Wrench : ArrowLeftRight;
    return (
      <li className="conv-turn conv-turn--note">
        <Icon size={12} aria-hidden />
        <span className="sr-only">{t.role === "call" ? "Tool call" : "Transfer"}: </span>
        <span className={t.role === "call" ? "conv-turn__call" : undefined} data-user-content>
          {t.role === "system" ? t.text[0].toUpperCase() + t.text.slice(1) : t.text}
        </span>
        <span className="sr-only">, {time}</span>
        {showTime && (
          <span className="conv-turn__time" aria-hidden>
            {time}
          </span>
        )}
      </li>
    );
  }
  const you = t.role === "user";
  return (
    <li className={cx("conv-turn", you ? "conv-turn--you" : "conv-turn--line")}>
      <span className="sr-only">
        {you ? "You" : lineName}, {time}:{" "}
      </span>
      <div className="conv-turn__bubble" data-user-content>
        {t.text}
      </div>
      {showTime && (
        <span className="conv-turn__time" aria-hidden>
          {time}
        </span>
      )}
    </li>
  );
}
