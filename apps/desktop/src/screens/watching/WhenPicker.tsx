/**
 * Owner: watching. When a schedule runs, without cron: five plain choices, each with only the
 * control it needs, and the result read back as the row will say it. The cron form is one click
 * further (“Use a cron expression”), and a stored schedule the plain choices can't say opens there.
 */
import { useId } from "react";

import { Button, cx, Field, Input, Select, SegmentedControl } from "../../ui";
import { cronWords } from "./model";
import { DAY_LONG, DAY_SHORT, type Plain, type PlainMode, plainWords, requestOf, type Unit } from "./when";

const MODES: { value: Exclude<PlainMode, "cron">; label: string }[] = [
  { value: "weekdays", label: "Weekdays" },
  { value: "daily", label: "Every day" },
  { value: "days", label: "Some days" },
  { value: "interval", label: "Every few hours" },
  { value: "once", label: "Once" },
];
const UNITS: { value: Unit; label: string }[] = [
  { value: "minutes", label: "minutes" },
  { value: "hours", label: "hours" },
  { value: "days", label: "days" },
];

export function WhenPicker({ value: p, onChange, problem, zone }: { value: Plain; onChange: (p: Plain) => void; problem?: string | null; zone: string }) {
  const id = useId();
  const set = (patch: Partial<Plain>) => onChange({ ...p, ...patch });
  const r = requestOf(p);
  const own = "problem" in r ? r.problem : null;
  const words = p.mode === "cron" ? (p.cron.trim() ? cronWords(p.cron.trim()) : null) : plainWords(p);
  const timed = p.mode === "weekdays" || p.mode === "daily" || p.mode === "days";

  return (
    <Field
      label="When"
      error={problem ?? (own && (p.mode === "cron" ? p.cron.trim() : true) ? own : undefined)}
      help={
        <span className="wat-when__readback" aria-live="polite">
          {words ? <strong>{words}</strong> : null}
          {words ? " · " : ""}
          {zone ? `in ${zone} time` : "in this computer’s time"}
        </span>
      }
    >
      <div className="wat-when">
        {p.mode !== "cron" ? (
          <>
            <SegmentedControl aria-label="How often" value={p.mode} onChange={(m) => set({ mode: m })} options={MODES} />
            <div className="wat-when__line">
              {timed && (
                <label className="wat-when__part">
                  <span>at</span>
                  <Input type="time" value={p.time} aria-label="Time" onChange={(e) => set({ time: e.target.value })} className="wat-when__time" />
                </label>
              )}
              {p.mode === "interval" && (
                <label className="wat-when__part">
                  <span>every</span>
                  <Input type="number" min={1} value={String(p.n)} aria-label="How many" onChange={(e) => set({ n: Number(e.target.value) })} className="wat-when__n" />
                  <Select aria-label="Unit" value={p.unit} onChange={(e) => set({ unit: e.target.value as Unit })} options={UNITS} size="md" />
                </label>
              )}
              {p.mode === "once" && (
                <label className="wat-when__part">
                  <span>on</span>
                  <Input type="datetime-local" value={p.date} aria-label="Date and time" onChange={(e) => set({ date: e.target.value })} />
                </label>
              )}
            </div>
            {p.mode === "days" && (
              <div className="wat-days" role="group" aria-label="Days">
                {DAY_SHORT.map((d, i) => {
                  const on = p.days.includes(i);
                  return (
                    <button
                      key={d}
                      type="button"
                      className={cx("wat-day", on && "is-on")}
                      aria-pressed={on}
                      aria-label={DAY_LONG[i]}
                      onClick={() => set({ days: on ? p.days.filter((x) => x !== i) : [...p.days, i] })}
                    >
                      {d}
                    </button>
                  );
                })}
              </div>
            )}
            <Button
              variant="ghost"
              size="sm"
              className="wat-when__switch"
              onClick={() => {
                const now = requestOf(p);
                set({ mode: "cron", cron: !("problem" in now) && now.kind === "cron" ? now.when : p.cron });
              }}
            >
              Use a cron expression
            </Button>
          </>
        ) : (
          <>
            <Input id={`${id}-cron`} mono value={p.cron} placeholder="0 9 * * 1-5" aria-label="Cron expression" onChange={(e) => set({ cron: e.target.value })} />
            <Button variant="ghost" size="sm" className="wat-when__switch" onClick={() => set({ mode: "weekdays" })}>
              Choose in plain words instead
            </Button>
          </>
        )}
      </div>
    </Field>
  );
}
