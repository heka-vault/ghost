/**
 * Owner: session. The live transcript in one centered reading column: Ghost's (or the
 * line's) words unboxed beside its glyph, the operator's in a soft bubble on the right,
 * consecutive turns grouped under one name and time, and quiet centered captions for the
 * session start, transfers between lines, settled changes and system notes.
 */
import { Check, Clock, TriangleAlert, Undo2, X } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import { formatTime, LineGlyph } from "../../ui";
import type { Decision, Item } from "./transcript";

export interface TranscriptProps {
  items: Item[];
  nameOf: (slug: string) => string;
  /** The line working on a reply right now, shown as a last “Thinking…” row. */
  thinkingAs: string | null;
  /** The settled change Undo would reverse right now, by key, for its inline Undo. */
  undoKey: string | null;
  onUndo: () => void;
  undoDisabled: boolean;
}

export function Transcript({ items, nameOf, thinkingAs, undoKey, onUndo, undoDisabled }: TranscriptProps) {
  const last = items[items.length - 1];
  const thinkingJoins = thinkingAs != null && last?.kind === "ghost" && last.slug === thinkingAs;
  // The name and time lead a reply only when the voice changes (the first reply, and the
  // first after a transfer); the rest of a back-and-forth reads as one conversation.
  let voice: string | null = null;
  const named = new Set<Item>();
  let typed = 0;
  let spoken = 0;
  items.forEach((it, i) => {
    if (it.kind === "transfer") voice = null;
    if (it.kind === "ghost") {
      if (it.slug !== voice) named.add(it);
      voice = it.slug;
    }
    if (it.kind === "you") {
      if (it.typed) typed += it.turns.length;
      else spoken += it.turns.length;
    }
  });
  const thinkingNamed = thinkingAs != null && !thinkingJoins && thinkingAs !== voice;
  // Only the exception says how it was said: typed turns in a spoken session, spoken ones in a typed one.
  const mostlyTyped = typed > spoken;
  return (
    <div className="session-transcript" role="log" aria-label="Transcript" aria-live="polite">
      {items.map((it) => {
        switch (it.kind) {
          case "started":
            return (
              <Caption key={it.key}>
                Session started <time dateTime={it.at}>{formatTime(it.at)}</time>
              </Caption>
            );
          case "transfer":
            return (
              <Caption key={it.key} glyph={<LineGlyph slug={it.to} name={nameOf(it.to)} size="sm" />} rule>
                {/* Without the line's own sentence (a switch seen only on the status stream) the marker keeps the same shape, timed by the new line's first words. */}
                <span data-user-content>{it.text || (it.to === "ghost" ? "Back to Ghost" : `Patched through to ${nameOf(it.to)}`)}</span>
                {it.at && (
                  <>
                    {" · "}
                    <time dateTime={it.at}>{formatTime(it.at)}</time>
                  </>
                )}
              </Caption>
            );
          case "note":
            return (
              <Caption key={it.key}>
                <span data-user-content>{it.text}</span> · <time dateTime={it.at}>{formatTime(it.at)}</time>
              </Caption>
            );
          case "decision":
            return <DecisionCaption key={it.key} d={it.decision} undo={undoKey === it.decision.key ? { onUndo, disabled: undoDisabled } : null} />;
          case "ghost":
            return (
              <Speaker key={it.key} slug={it.slug} name={nameOf(it.slug)} at={it.turns[0].at} named={named.has(it)}>
                {/* Speaking shows once, on the composer's presence; the words themselves stay plain. */}
                {it.turns.map((t, i) => (
                  <p key={i} className="session-msg" data-user-content>
                    {t.text}
                  </p>
                ))}
                {thinkingJoins && it === last && <Thinking />}
              </Speaker>
            );
          case "you":
            return (
              <div key={it.key} className="session-you">
                <span className="sr-only">You {it.typed ? "typed" : "said"}:</span>
                {it.turns.map((t, i) => (
                  <p key={i} className="session-bubble" data-user-content>
                    {t.text}
                  </p>
                ))}
                {it.typed !== mostlyTyped && (
                  <p className="session-you__meta" aria-hidden>
                    {it.typed ? "Typed" : "Spoken"} · <time dateTime={it.turns[0].at}>{formatTime(it.turns[0].at)}</time>
                  </p>
                )}
              </div>
            );
        }
      })}
      {thinkingAs != null && !thinkingJoins && (
        <Speaker slug={thinkingAs} name={nameOf(thinkingAs)} named={thinkingNamed}>
          <Thinking />
        </Speaker>
      )}
    </div>
  );
}

function Speaker({ slug, name, at, named, children }: { slug: string; name: string; at?: string; named: boolean; children: ReactNode }) {
  return (
    <div className={named ? "session-speaker is-named" : "session-speaker"}>
      {named ? <LineGlyph slug={slug} name={name} size="md" className="session-speaker__glyph" /> : <span />}
      <div className="session-speaker__body">
        {named ? (
          <p className="session-speaker__head">
            <span className="session-speaker__name">{name}</span>
            {at && (
              <time className="session-speaker__time" dateTime={at}>
                {formatTime(at)}
              </time>
            )}
          </p>
        ) : (
          <span className="sr-only">{name}:</span>
        )}
        {children}
      </div>
    </div>
  );
}

/** A quiet live row while the model works: a shimmer on the words (still under reduced motion), and the wait after 3 seconds. */
function Thinking() {
  const [start] = useState(() => Date.now());
  const [now, setNow] = useState(start);
  useEffect(() => {
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, []);
  const s = Math.floor((now - start) / 1000);
  return (
    <p className="session-msg session-thinking">
      <span className="session-thinking__text">Thinking…</span>
      {/* Visual only: the log would read every tick aloud. */}
      {s >= 3 && (
        <span className="session-thinking__time" aria-hidden>
          {s < 60 ? `${s}s` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`}
        </span>
      )}
    </p>
  );
}

const DECIDED: Record<Decision["outcome"], { icon: ReactNode; label: (d: Decision) => string }> = {
  confirmed: { icon: <Check />, label: (d) => (d.byVoice ? "You said yes" : "You confirmed") },
  canceled: { icon: <X />, label: (d) => (d.byVoice ? "You said no" : "You canceled") },
  expired: { icon: <Clock />, label: () => "Expired" },
  failed: { icon: <TriangleAlert />, label: () => "Didn’t go through" },
  undone: { icon: <Undo2 />, label: () => "You undid" },
};

/** “You confirmed · close the issue on ghost-mock/app#163 · 4:05 PM”, with Undo while it is the change Undo would reverse. */
function DecisionCaption({ d, undo }: { d: Decision; undo: { onUndo: () => void; disabled: boolean } | null }) {
  const shown = d.undone ? { icon: <Undo2 />, label: () => "You confirmed, then undid it" } : DECIDED[d.outcome];
  return (
    <Caption glyph={<span className="session-caption__icon">{shown.icon}</span>}>
      {shown.label(d)} · <span data-user-content>{d.what}</span>
      <span className="session-nowrap">
        {" · "}
        <time dateTime={d.at}>{formatTime(d.at)}</time>
      </span>
      {undo && !d.undone && (
        <>
          {" "}
          <button type="button" className="session-caption__undo" onClick={undo.onUndo} disabled={undo.disabled} aria-label={`Undo: ${d.what}`}>
            Undo
          </button>
        </>
      )}
    </Caption>
  );
}

function Caption({ children, glyph, rule }: { children: ReactNode; glyph?: ReactNode; rule?: boolean }) {
  return (
    <p className={rule ? "session-caption session-caption--rule" : "session-caption"}>
      <span className="session-caption__inner">
        {glyph}
        <span>{children}</span>
      </span>
    </p>
  );
}
