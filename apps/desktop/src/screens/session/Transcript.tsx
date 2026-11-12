/**
 * Owner: session. The record: the session as one timeline on a rail, not a chat.
 *
 * Every row is a moment on the rail: a time in the gutter when time matters (the session's
 * start, after a pause; any row's on hover or focus), a mark on the rail that says what kind
 * of moment it is, and the words. Ghost (or the agent) is its glyph; the operator is a ring,
 * a mic inside for spoken, keys for typed; the work Ghost did between two turns is one quiet
 * row that opens to its calls; a change made, a change settled, a question, a cut, an error,
 * hold and the session's end each have their own mark. Ghost's words are the brightest thing
 * on the page; the operator's sit a step back, since they know what they said.
 *
 * The reply being said shows which words have been said (`said.ts`): said ones in full
 * color, the rest dim, moving word by word with Ghost's voice. A reply the operator cut off
 * keeps the dim part and a mark where the cut came. Screen readers get each turn once, whole:
 * the split is visual only.
 */
import { joinSpoken, secondsBetween, useGhost } from "@ghost/ui";
import type { QuestionView, Turn } from "@ghost/client-ts";
import { Check, ChevronRight, Clock, Copy, Hand, Keyboard, Mic, Pause, PhoneOff, TriangleAlert, Volume2, Wrench, X } from "lucide-react";
import { type ReactNode, useEffect, useState } from "react";

import { Button, formatDateTime, formatTime, IconButton, LineGlyph, cx } from "../../ui";
import type { Saying } from "./live";
import { cutReplies, gapText, lengthText } from "./timeline";
import type { Decision, Item, LineInfo } from "./transcript";

export interface TranscriptProps {
  items: Item[];
  nameOf: (slug: string) => string;
  /** The agent working on a reply right now, shown as a last “Working” row. */
  thinkingAs: string | null;
  /** How much of each reply was said, by its time (`useSaying`). */
  saying?: Record<string, Saying>;
  /** The agents, for replaying a reply in its own voice. */
  lines?: LineInfo[];
  /** Replay is offered only with no session open: Ghost's voice would reach the microphone. */
  canReplay?: boolean;
  /** Answer a question with one of its choices (ADR 0051). */
  onAnswer?: (q: QuestionView, choice: string) => Promise<void>;
}

export function Transcript({ items, nameOf, thinkingAs, saying = {}, lines = [], canReplay = false, onAnswer }: TranscriptProps) {
  const last = items[items.length - 1];
  const cut = cutReplies(items);
  // The name leads a reply only when the voice changes (the first reply, the first after a
  // transfer); the rest of a back-and-forth reads as one conversation.
  let voice: string | null = null;
  const named = new Set<Item>();
  // The time shows on the rail at a session's start and after a pause; otherwise on hover.
  const timed = new Set<Item>();
  let typed = 0;
  let spoken = 0;
  let fresh = true;
  for (const it of items) {
    if (it.kind === "transfer" || it.kind === "started") voice = null;
    if (it.kind === "started" || it.kind === "gap") fresh = true;
    else if (it.kind === "you" || it.kind === "ghost" || it.kind === "run") {
      if (fresh) timed.add(it);
      fresh = false;
    }
    if (it.kind === "ghost") {
      if (it.slug !== voice) named.add(it);
      voice = it.slug;
    }
    if (it.kind === "you") {
      if (it.typed) typed += it.turns.length;
      else spoken += it.turns.length;
    }
  }
  const thinkingJoins = thinkingAs != null && last?.kind === "ghost" && last.slug === thinkingAs;
  const thinkingNamed = thinkingAs != null && !thinkingJoins && thinkingAs !== voice;
  const mostlyTyped = typed > spoken;
  const voiceOf = (slug: string) => {
    const l = lines.find((x) => x.slug === slug) ?? lines.find((x) => x.slug === "ghost");
    return l?.voice ? { voice: l.voice, speed: l.speed ?? 1 } : null;
  };

  return (
    <div className="thread" role="log" aria-label="Transcript" aria-live="polite">
      {items.map((it) => {
        switch (it.kind) {
          case "started":
            return (
              <Divider key={it.key} at={it.at}>
                Session started · <time dateTime={it.at}>{it.dated && !sameDay(it.at) ? formatDateTime(it.at) : formatTime(it.at)}</time>
              </Divider>
            );
          case "ended":
            return (
              <Row key={it.key} mark={<PhoneOff />} tone="quiet" at={it.at}>
                <p className="thread-caption">Session ended · {lengthText(it.secs)}</p>
              </Row>
            );
          case "gap":
            return (
              <div key={it.key} className="thread-gap" aria-hidden>
                <span className="thread-gap__text">{gapText(it.secs)}</span>
              </div>
            );
          case "transfer":
            return (
              <Row key={it.key} mark={<LineGlyph slug={it.to} name={nameOf(it.to)} size="sm" />} at={it.at ?? undefined}>
                <p className="thread-caption">
                  <span data-user-content>{it.text || (it.to === "ghost" ? "Back to Ghost" : `Transferred to ${nameOf(it.to)}`)}</span>
                </p>
              </Row>
            );
          case "note":
            return (
              <Row key={it.key} mark={<Dot />} tone="quiet" at={it.at}>
                <p className="thread-caption" data-user-content>
                  {it.text}
                </p>
              </Row>
            );
          case "decision":
            return <DecisionRow key={it.key} d={it.decision} />;
          case "ghost": {
            const v = voiceOf(it.slug);
            return (
              <Row
                key={it.key}
                mark={<LineGlyph slug={it.slug} name={nameOf(it.slug)} size="md" />}
                at={it.turns[0].at}
                timed={timed.has(it)}
                kind="ghost"
                actions={<ReplyActions text={it.turns.map((t) => t.text).join(" ")} voice={canReplay ? v : null} />}
              >
                {named.has(it) ? <p className="thread-name">{nameOf(it.slug)}</p> : <span className="sr-only">{nameOf(it.slug)}:</span>}
                {it.turns.map((t, i) => (
                  <Reply key={i} text={t.text} saying={saying[t.at]} cutHere={cut.has(it.key) && i === it.turns.length - 1} />
                ))}
                {thinkingJoins && it === last && <Working />}
              </Row>
            );
          }
          case "you":
            return (
              <Row key={it.key} mark={<YouMark typed={it.typed} />} at={it.turns[0].at} timed={timed.has(it)} kind="you">
                <span className="sr-only">You {it.typed ? "typed" : "said"}:</span>
                {joinSpoken(it.turns).map((t, i) => (
                  <p key={i} className="thread-you" data-user-content>
                    {t.text}
                  </p>
                ))}
                {it.typed !== mostlyTyped && (
                  <p className="thread-meta" aria-hidden>
                    {it.typed ? "Typed" : "Spoken"}
                  </p>
                )}
              </Row>
            );
          case "run":
            return <RunRow key={it.key} label={it.label} turns={it.turns} />;
          case "work":
            return <WorkRow key={it.key} item={it} />;
          case "cut":
            return (
              <Row key={it.key} mark={<Hand />} tone="quiet" at={it.at}>
                <p className="thread-caption">You cut in</p>
              </Row>
            );
          case "error":
            return (
              <Row key={it.key} mark={<TriangleAlert />} tone="danger" at={it.at}>
                <p className="thread-caption thread-caption--danger">That turn didn’t finish. Ghost’s log says why.</p>
              </Row>
            );
          case "change":
            return (
              <Row key={it.key} mark={it.ok ? <Check /> : <TriangleAlert />} tone={it.ok ? "ok" : "danger"} at={it.at}>
                <p className="thread-caption">
                  {it.ok ? "Done" : "Didn’t go through"} · <span data-user-content>{it.summary}</span>
                </p>
              </Row>
            );
          case "question":
            return <QuestionRow key={it.key} q={it.question} nameOf={nameOf} onAnswer={onAnswer} />;
          case "hold":
            return (
              <Row key={it.key} mark={<Pause />} tone="quiet" at={it.at}>
                <p className="thread-caption">
                  On hold
                  {it.back && <> · back after {lengthText(secondsBetween(it.at, it.back))}</>}
                </p>
              </Row>
            );
        }
      })}
      {thinkingAs != null && !thinkingJoins && (
        <Row mark={<LineGlyph slug={thinkingAs} name={nameOf(thinkingAs)} size="md" />} kind="ghost">
          {thinkingNamed && <p className="thread-name">{nameOf(thinkingAs)}</p>}
          <Working />
        </Row>
      )}
    </div>
  );
}

// ---------------------------------------------------------------- rows

type Tone = "quiet" | "ok" | "warn" | "danger";

function Row({ mark, at, timed, kind, tone, actions, children }: { mark: ReactNode; at?: string; timed?: boolean; kind?: "ghost" | "you"; tone?: Tone; actions?: ReactNode; children: ReactNode }) {
  return (
    <div className={cx("thread-row", kind && `thread-row--${kind}`, tone && `thread-row--${tone}`, timed && "is-timed")}>
      <div className="thread-row__time">{at && <time dateTime={at}>{formatTime(at)}</time>}</div>
      <div className="thread-row__mark" aria-hidden>
        {mark}
      </div>
      <div className="thread-row__body">{children}</div>
      {actions && <div className="thread-row__actions">{actions}</div>}
    </div>
  );
}

function Divider({ at, children }: { at: string; children: ReactNode }) {
  return (
    <div className="thread-divider" data-at={at}>
      <span className="thread-divider__text">{children}</span>
    </div>
  );
}

function Dot() {
  return <span className="thread-dot" />;
}

function YouMark({ typed }: { typed: boolean }) {
  return <span className="thread-you-mark">{typed ? <Keyboard /> : <Mic />}</span>;
}

/** One of Ghost's turns: plain, or split where the saying is (said bright, the rest dim). */
function Reply({ text, saying, cutHere }: { text: string; saying?: Saying; cutHere: boolean }) {
  if (!saying) return <p className="thread-ghost" data-user-content>{text}</p>;
  const said = text.slice(0, saying.chars);
  const rest = text.slice(saying.chars);
  return (
    <p className={cx("thread-ghost", saying.cut ? "is-cut" : "is-saying")} data-user-content>
      <span className="sr-only">{text}</span>
      <span aria-hidden>
        <span className="thread-said">{said}</span>
        {(saying.cut || cutHere) && <span className="thread-cut" title="You cut in here" />}
        <span className="thread-unsaid">{rest}</span>
      </span>
    </p>
  );
}

/**
 * On hover or focus: copy a reply, or hear it again in its agent's voice (only with no session
 * open, and only a reply short enough for the voice sample: 200 characters).
 */
function ReplyActions({ text, voice }: { text: string; voice: { voice: string; speed: number } | null }) {
  const { client } = useGhost();
  const [state, setState] = useState<"idle" | "playing" | "copied">("idle");
  const replay = voice && text.length <= 200;
  return (
    <>
      {replay && (
        <IconButton
          size="sm"
          variant="ghost"
          label={state === "playing" ? "Saying it again…" : "Say it again"}
          icon={<Volume2 />}
          loading={state === "playing"}
          onClick={async () => {
            setState("playing");
            try {
              await client.sampleVoice(voice.voice, voice.speed, text);
            } finally {
              setState("idle");
            }
          }}
        />
      )}
      <IconButton
        size="sm"
        variant="ghost"
        label={state === "copied" ? "Copied" : "Copy"}
        icon={state === "copied" ? <Check /> : <Copy />}
        onClick={async () => {
          await navigator.clipboard?.writeText(text).catch(() => {});
          setState("copied");
          window.setTimeout(() => setState("idle"), 1200);
        }}
      />
    </>
  );
}

/** The work between two turns: one row, “Used GitHub · 2 calls · 1.4 s”, opened to each call. */
function WorkRow({ item }: { item: Extract<Item, { kind: "work" }> }) {
  const [open, setOpen] = useState(false);
  const running = item.calls.some((c) => !c.endedAt);
  const now = useTicking(running);
  const apps = [...new Set(item.calls.map((c) => c.app ?? "an app"))].map(appTitle);
  const end = running ? now : item.calls.reduce((m, c) => (c.endedAt && c.endedAt > m ? c.endedAt : m), item.at);
  const secs = secondsBetween(item.at, end);
  const n = item.calls.length;
  const what = n === 0 ? "Handed to the main model" : `${running ? "Using" : "Used"} ${list(apps)}`;
  return (
    <Row mark={<Wrench />} tone="quiet" at={item.at}>
      <button type="button" className="thread-work" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
        <span className={cx("thread-work__what", running && "is-running")}>{what}</span>
        <span className="thread-work__meta">
          {n > 0 && <>{n === 1 ? "1 call" : `${n} calls`} · </>}
          {seconds(secs)}
        </span>
        <ChevronRight className="thread-work__chev" aria-hidden />
      </button>
      {open && (
        <ul className="thread-calls">
          {item.handover && (
            <li className="thread-call">
              <span className="thread-call__state">
                <Dot />
              </span>
              <span className="thread-call__name">The fast model handed this turn to the main one</span>
              <span className="thread-call__time">{seconds(item.handover.secs)}</span>
            </li>
          )}
          {item.calls.map((c) => (
            <li key={c.id} className="thread-call">
              <span className={cx("thread-call__state", !c.endedAt && "is-running")}>{c.endedAt ? <Check /> : <Dot />}</span>
              <span className="thread-call__name">
                <span className="thread-call__app">{appTitle(c.app ?? "app")}</span> {c.tool}
                <span className="thread-call__id t-mono">{c.name}</span>
              </span>
              <span className="thread-call__time">{c.endedAt ? seconds(secondsBetween(c.startedAt, c.endedAt)) : "running"}</span>
            </li>
          ))}
          <li className="thread-call thread-call--note">What came back went to the model, not this screen. Changes are in the audit log.</li>
        </ul>
      )}
    </Row>
  );
}

/** A scheduled run's or an asked turn's entry: its label, its answer, its instruction on open. */
function RunRow({ label, turns }: { label: string; turns: Turn[] }) {
  const [open, setOpen] = useState(false);
  const answer = [...turns].reverse().find((t) => t.role === "assistant");
  const asked = turns.find((t) => t.role !== "assistant");
  return (
    <Row mark={<Clock />} at={turns[0].at} timed tone="quiet">
      <p className="thread-name">{label}</p>
      {answer && (
        <p className="thread-ghost" data-user-content>
          {answer.text}
        </p>
      )}
      {asked && (
        <>
          <button type="button" className="thread-work thread-work--plain" aria-expanded={open} onClick={() => setOpen((o) => !o)}>
            <span className="thread-work__what">What it was asked</span>
            <ChevronRight className="thread-work__chev" aria-hidden />
          </button>
          {open && (
            <p className="thread-you" data-user-content>
              {asked.text}
            </p>
          )}
        </>
      )}
    </Row>
  );
}

/** A question an agent asked (ADR 0051): its words are the asker's, shown as “Ledger asks: …”. */
function QuestionRow({ q, nameOf, onAnswer }: { q: QuestionView; nameOf: (slug: string) => string; onAnswer?: (q: QuestionView, choice: string) => Promise<void> }) {
  const [busy, setBusy] = useState<string | null>(null);
  const who = q.asked_by.startsWith("agent:") ? nameOf(q.asked_by.slice(6)) : q.asked_by.replace(/^client:/, "");
  const open = q.state === "open";
  return (
    <Row mark={<span className="thread-ask-mark">?</span>} tone={open ? "warn" : "quiet"} at={q.asked_at}>
      <p className="thread-ask">
        <span className="thread-ask__who">{who} asks:</span> <span data-user-content>{q.question}</span>
      </p>
      {q.detail && (
        <p className="thread-caption" data-user-content>
          {q.detail}
        </p>
      )}
      {open && q.choices.length > 0 && onAnswer ? (
        <div className="thread-choices" role="group" aria-label={`Answer ${who}`}>
          {q.choices.map((c) => (
            <Button
              key={c}
              size="sm"
              variant="secondary"
              loading={busy === c}
              disabled={busy !== null}
              onClick={async () => {
                setBusy(c);
                try {
                  await onAnswer(q, c);
                } finally {
                  setBusy(null);
                }
              }}
            >
              <span data-user-content>{c}</span>
            </Button>
          ))}
        </div>
      ) : open ? (
        <p className="thread-caption">Answer by voice, or on What needs you.</p>
      ) : (
        <p className="thread-caption">
          {q.state === "answered" ? (
            <>
              You answered{q.answered_via === "voice" ? " by voice" : ""}: <span data-user-content>{q.choice ?? q.answer}</span>
            </>
          ) : q.state === "expired" ? (
            "No answer in time"
          ) : (
            "Not answered"
          )}
        </p>
      )}
    </Row>
  );
}

const DECIDED: Record<Decision["outcome"], { icon: ReactNode; tone: Tone; label: (d: Decision) => string }> = {
  confirmed: { icon: <Check />, tone: "ok", label: (d) => (d.byVoice ? "You said yes" : "You confirmed") },
  canceled: { icon: <X />, tone: "quiet", label: (d) => (d.byVoice ? "You said no" : "You canceled") },
  expired: { icon: <Clock />, tone: "quiet", label: () => "Expired" },
  failed: { icon: <TriangleAlert />, tone: "danger", label: () => "Didn’t go through" },
};

/** “You confirmed · close the issue on ghost-mock/app#163”. */
function DecisionRow({ d }: { d: Decision }) {
  const shown = DECIDED[d.outcome];
  return (
    <Row mark={shown.icon} tone={shown.tone} at={d.at}>
      <p className="thread-caption">
        {shown.label(d)} · <span data-user-content>{d.what}</span>
      </p>
    </Row>
  );
}

/** The model is working: a quiet live line, the wait after 3 seconds. */
function Working() {
  const [start] = useState(() => Date.now());
  const now = useTicking(true);
  const s = Math.floor((new Date(now).getTime() - start) / 1000);
  return (
    <p className="thread-ghost thread-working">
      <span className="thread-working__text">Working on it…</span>
      {s >= 3 && (
        <span className="thread-working__time" aria-hidden>
          {s < 60 ? `${s}s` : `${Math.floor(s / 60)}:${String(s % 60).padStart(2, "0")}`}
        </span>
      )}
    </p>
  );
}

// ---------------------------------------------------------------- helpers

/** The time now, ticking once a second while `on`. */
function useTicking(on: boolean): string {
  const [now, setNow] = useState(() => new Date().toISOString());
  useEffect(() => {
    if (!on) return;
    const t = window.setInterval(() => setNow(new Date().toISOString()), 1000);
    return () => window.clearInterval(t);
  }, [on]);
  return now;
}

function sameDay(iso: string): boolean {
  return new Date(iso).toDateString() === new Date().toDateString();
}

function seconds(s: number): string {
  return s < 10 ? `${s.toFixed(1)} s` : `${Math.round(s)} s`;
}

/** An app's name as a title: `github` → “GitHub”, `t3` → “T3”, `google_calendar` → “Google calendar”. */
export function appTitle(app: string): string {
  const known: Record<string, string> = { github: "GitHub", ghost: "Ghost", gitlab: "GitLab", linear: "Linear", slack: "Slack", notion: "Notion" };
  if (known[app]) return known[app];
  const words = app.replace(/[_-]+/g, " ");
  return words.charAt(0).toUpperCase() + words.slice(1);
}

function list(xs: string[]): string {
  if (xs.length <= 1) return xs[0] ?? "";
  return `${xs.slice(0, -1).join(", ")} and ${xs[xs.length - 1]}`;
}
