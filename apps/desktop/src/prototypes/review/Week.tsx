/**
 * Owner: review. The weekly summary (FR-022): what got decided, what it unblocked, how long
 * things waited, and when Ghost reads it aloud. Glanceable; the numbers compare with the
 * week before rather than with a goal.
 */
import { CalendarDays, ChevronRight, ExternalLink } from "lucide-react";
import { useState } from "react";

import type { ReviewFixture, ReviewWeek } from "../../design/mock/fixtures/review";
import { Badge, Card, EmptyState, Field, formatTime, Section, Select, quoted, statusLabel, statusTone } from "../../ui";
import { githubUrl, keyOf, spoken, spokenRef, WEEK_PHRASE } from "./model";

const days = (h: number) => (h < 36 ? `${Math.round(h)} h` : `${(h / 24).toFixed(1)} days`);

function dayLabel(iso: string): string {
  const d = new Date(iso);
  const today = new Date();
  const diff = Math.round((new Date(today.toDateString()).getTime() - new Date(d.toDateString()).getTime()) / 86_400_000);
  if (diff === 0) return "Today";
  if (diff === 1) return "Yesterday";
  return d.toLocaleDateString("en-US", { weekday: "long" });
}

const plural = (n: number, one: string, many = `${one}s`) => `${n} ${n === 1 ? one : many}`;

/** One sentence for the week, the focal point: what got decided, what it freed, how waits moved. */
function hero(w: ReviewWeek): string {
  const now = days(w.medianWaitHours);
  const before = days(w.medianWaitPrevHours);
  const trend =
    w.medianWaitHours < w.medianWaitPrevHours ? `down from ${before}` : w.medianWaitHours > w.medianWaitPrevHours ? `up from ${before}` : "the same as last week";
  const unblocked = w.unblocked ? ` and unblocked ${plural(w.unblocked, "other")}` : "";
  return `You decided ${plural(w.decided, "issue")}${unblocked}. Issues typically waited ${now} for you, ${trend}.`;
}

export interface WeekProps {
  week: ReviewWeek;
  waiting: number;
  /** When the longest-waiting issue started waiting on the operator (ISO). */
  oldest: string | null;
  issues: ReviewFixture["issues"];
  inSession: boolean;
  canTalk: boolean;
  onOpen: (key: string) => void;
}

export function Week({ week, waiting, oldest, issues, inSession, canTalk, onOpen }: WeekProps) {
  const say = quoted(spoken(WEEK_PHRASE, inSession));
  const [cadence, setCadence] = useState(week.cadence);
  const groups: { day: string; items: ReviewWeek["items"] }[] = [];
  for (const it of week.items) {
    const day = dayLabel(it.at);
    const g = groups.at(-1);
    if (g && g.day === day) g.items.push(it);
    else groups.push({ day, items: [it] });
  }
  const inQueue = new Set(issues.map(keyOf));
  const since = oldest ? new Date(oldest).toLocaleDateString(undefined, { month: "short", day: "numeric" }) : null;
  const still =
    waiting === 0
      ? "Nothing is waiting on you now."
      : `${waiting === 1 ? "One issue is" : `${waiting} issues are`} still waiting on you${since ? `, the oldest since ${since}` : ""}.`;

  return (
    <div className="rev-week">
      {week.decided === 0 ? (
        <Card padding="lg" className="rev-week__empty">
          <EmptyState
            icon={<CalendarDays />}
            title={cadence === "off" ? "Your weekly summary shows up once you’ve decided a few" : `Your first weekly summary comes ${cadence === "monday" ? "Monday" : "Friday"}`}
            description={<>Once you’ve made a few decisions with Ghost, this shows what they unblocked and how long things waited.{canTalk && <> Say {say} any time.</>}</>}
          />
        </Card>
      ) : (
        <>
          <div className="rev-week__lede">
            <p className="t-title rev-week__hero">{hero(week)}</p>
            <p className="t-small t-2 rev-week__still">{still}</p>
          </div>

          <Section title="What you decided" description={week.range}>
            {/* Every row opens something: the issue in the queue when it's still there, otherwise
                the issue on GitHub. The trailing glyph says which. */}
            <Card padding="none" className="rev-week__list">
              {groups.map((g) => (
                <div key={g.day} role="group" aria-labelledby={`rev-day-${g.day}`} className="rev-week__group">
                  <p id={`rev-day-${g.day}`} className="rev-week__day t-overline">{g.day}</p>
                  <ul className="rev-week__rows">
                    {g.items.map((it) => {
                      const k = keyOf(it);
                      const here = inQueue.has(k);
                      const body = (
                        <>
                          <span className="rev-wrow__text">
                            <span className="rev-wrow__title" data-user-content>{it.title}</span>
                            <span className="rev-wrow__meta">
                              <span className="rev-wrow__ref t-num">{spokenRef(it)}</span>
                              <span className="rev-row__sep" aria-hidden>·</span>
                              <span className="rev-wrow__what">{it.text}</span>
                              {it.unblocked > 0 && <span className="rev-wrow__extra"> · unblocked {it.unblocked}</span>}
                              <span className="rev-wrow__time t-num"> · {formatTime(it.at)}</span>
                            </span>
                          </span>
                          {it.status !== "executed" && <Badge size="sm" dot tone={statusTone(it.status)}>{statusLabel(it.status)}</Badge>}
                          {here ? <ChevronRight className="rev-wrow__glyph" aria-hidden /> : <ExternalLink className="rev-wrow__glyph" aria-hidden />}
                        </>
                      );
                      return (
                        <li key={`${k}-${it.at}`}>
                          {here ? (
                            <button type="button" className="rev-wrow" onClick={() => onOpen(k)}>
                              {body}
                            </button>
                          ) : (
                            <a className="rev-wrow" href={githubUrl(it)} target="_blank" rel="noreferrer">
                              {body}
                              <span className="sr-only"> (opens on GitHub)</span>
                            </a>
                          )}
                        </li>
                      );
                    })}
                  </ul>
                </div>
              ))}
            </Card>
            {week.items.length < week.decided && (
              <p className="t-small t-3 rev-week__more">
                The latest {week.items.length} of {week.decided}. The rest are in the audit log.
              </p>
            )}
          </Section>
        </>
      )}

      <Section
        title={
          <span className="rev-week__title">
            Read aloud <Badge size="sm" variant="outline">Prototype</Badge>
          </span>
        }
        description="Ghost reads you a short version in the main line’s voice."
      >
        <Card padding="none" className="rev-week__read">
          <div className="rev-week__field">
            <Field
              layout="inline"
              label="When Ghost reads it"
              help={canTalk ? <>Or say {say} any time.</> : undefined}
              controlWidth={240}
            >
              <Select
                value={cadence}
                onChange={(e) => setCadence(e.target.value as ReviewWeek["cadence"])}
                options={[
                  { value: "friday", label: "Fridays at 4 PM" },
                  { value: "monday", label: "Mondays at 9 AM" },
                  { value: "off", label: "Only when I ask" },
                ]}
              />
            </Field>
          </div>
        </Card>
      </Section>
    </div>
  );
}
