/**
 * One line under an agent on the roster: running now (with its clock), or what it last did on
 * its own and what it said, and when it next runs. Empty for an agent that never ran.
 */
import { useEffect, useState } from "react";

import { useToday } from "../../app/today";
import { relativeTime } from "../../ui";
import { clock } from "../today/model";
import { untilWords } from "../watching/model";
import { activityOf } from "./roster";

const ZONE = Intl.DateTimeFormat().resolvedOptions().timeZone;

export function RosterLine({ slug }: { slug: string }) {
  const { records, schedules } = useToday();
  const a = activityOf(slug, records.runs, schedules, ZONE);
  const [now, setNow] = useState(() => Date.now());
  useEffect(() => {
    if (!a.running) return;
    const t = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(t);
  }, [a.running]);
  if (!a.running && !a.last && !a.next) return null;
  return (
    <span className="conv-roster">
      {a.running ? (
        <span className="conv-roster__now">
          <span className="conv-roster__dot" aria-hidden />
          Running <span data-user-content>{a.running.name}</span> · <span className="t-num">{clock(a.running.started_at ?? a.running.scheduled_for, now)}</span>
        </span>
      ) : a.last ? (
        <span className="conv-roster__last">
          Ran <span data-user-content>{a.last.run.name}</span> {relativeTime(a.last.run.finished_at!)}
          {a.last.said && (
            <>
              {": "}
              <span className="conv-roster__said" data-user-content>
                {a.last.said}
              </span>
            </>
          )}
        </span>
      ) : null}
      {a.next && (
        <span className="conv-roster__next">
          Next: {a.next.when.charAt(0).toLowerCase() + a.next.when.slice(1)}, {untilWords(a.next.at)}
        </span>
      )}
    </span>
  );
}
