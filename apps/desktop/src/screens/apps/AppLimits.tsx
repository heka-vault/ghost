/**
 * Owner: apps. An app's own limits in apps.json (ADR 0046), on its page: how long a call may
 * take, how long connecting may take, how much of a result the model reads whole, and the
 * tools offered by name on every turn even past the limits on how many tools a turn carries.
 * Empty is Ghost's default, shown as the placeholder. The four are one draft, committed with
 * the page's save bar (`PUT /apps/{name}/limits`); the app connects again with them.
 */
import type { AppView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { type ReactNode, useEffect, useMemo, useRef, useState } from "react";

import { Card, Checkbox, Field, Input, SaveBar, Section, errorText, useLive, useToast } from "../../ui";
import { toolLabel } from "./model";

interface Draft {
  timeout: string;
  connect: string;
  chars: string;
  pinned: string[];
}

const draftOf = (a: AppView): Draft => ({
  timeout: a.limits.timeout?.toString() ?? "",
  connect: a.limits.connect_timeout?.toString() ?? "",
  chars: a.limits.result_chars?.toString() ?? "",
  pinned: [...a.limits.pinned],
});

const same = (x: Draft, y: Draft) =>
  x.timeout === y.timeout && x.connect === y.connect && x.chars === y.chars && x.pinned.length === y.pinned.length && x.pinned.every((p) => y.pinned.includes(p));

/** A whole number within bounds, or empty for the default; what is wrong otherwise. */
function numberProblem(raw: string, lo: number, hi: number, unit: string): string | null {
  if (raw.trim() === "") return null;
  const n = Number(raw);
  if (!Number.isInteger(n) || n < lo || n > hi) return `A whole number of ${unit} from ${lo.toLocaleString("en-US")} to ${hi.toLocaleString("en-US")}, or empty for Ghost’s default.`;
  return null;
}
const numberOf = (raw: string) => (raw.trim() === "" ? null : Number(raw));

export function useAppLimits(app: AppView | null, onSaved: (a: AppView) => void): { section: ReactNode; dock: ReactNode } {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const saved = useMemo(() => (app ? draftOf(app) : null), [app]);
  const [draft, setDraft] = useState<Draft | null>(saved);
  const [busy, setBusy] = useState(false);
  const [saveError, setSaveError] = useState<string | null>(null);

  // A fresh read of the app (another save, the file changed) replaces an untouched draft.
  const last = useRef<Draft | null>(saved);
  useEffect(() => {
    setDraft((d) => (d == null || last.current == null || same(d, last.current) ? saved : d));
    last.current = saved;
  }, [saved]);

  const dirty = draft != null && saved != null && !same(draft, saved);
  const problems = draft
    ? {
        timeout: numberProblem(draft.timeout, 1, 3600, "seconds"),
        connect: numberProblem(draft.connect, 1, 600, "seconds"),
        chars: numberProblem(draft.chars, 1000, 100_000, "characters"),
      }
    : { timeout: null, connect: null, chars: null };
  const invalid = Boolean(problems.timeout || problems.connect || problems.chars);

  const save = async () => {
    if (!app || !draft || busy || invalid || !online) return;
    setBusy(true);
    setSaveError(null);
    try {
      const a = await client.setAppLimits(app.name, {
        timeout: numberOf(draft.timeout),
        connect_timeout: numberOf(draft.connect),
        result_chars: numberOf(draft.chars),
        pinned: draft.pinned,
      });
      setDraft(draftOf(a));
      onSaved(a);
      toast({ tone: "ok", title: "Saved", description: `${app.title} connects again with them now.` });
    } catch (e) {
      setSaveError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  // Ctrl S saves, as on every page with a save bar.
  useEffect(() => {
    if (!dirty) return;
    const onKey = (e: KeyboardEvent) => {
      if ((e.ctrlKey || e.metaKey) && e.key.toLowerCase() === "s") {
        e.preventDefault();
        void save();
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  });

  if (!app || !draft) return { section: null, dock: null };
  const l = app.limits;
  const set = <K extends keyof Draft>(k: K, v: Draft[K]) => setDraft((d) => (d ? { ...d, [k]: v } : d));
  // The tools to pin: what it offers now, and any pinned name it doesn't offer (kept, said so).
  const tools = app.actions.map((a) => a.name);
  const gone = draft.pinned.filter((p) => !tools.includes(p));

  const section = (
    <Section
      id="apps-limits"
      title="Limits"
      description={`How long Ghost waits for ${app.title}, how much of what it returns the model reads, and which of its tools every turn carries. Empty is Ghost’s default.`}
    >
      <Card list>
        <div className="apps-limit">
          <Field layout="inline" label="A call may take" detail="timeout" help="Seconds before a call is stopped and the model is told." error={problems.timeout} controlWidth={160}>
            <Input value={draft.timeout} inputMode="numeric" placeholder={String(l.default_timeout)} trailing="s" onChange={(e) => set("timeout", e.target.value)} />
          </Field>
        </div>
        <div className="apps-limit">
          <Field layout="inline" label="Connecting may take" detail="connect_timeout" help="Seconds to connect, or to start it on this computer." error={problems.connect} controlWidth={160}>
            <Input value={draft.connect} inputMode="numeric" placeholder={String(l.default_connect_timeout)} trailing="s" onChange={(e) => set("connect", e.target.value)} />
          </Field>
        </div>
        <div className="apps-limit">
        <Field
          layout="inline"
          label="Results read whole"
          detail="result_chars"
          help="Characters of a result the model is given; past that it is cut at the end and told how much was left out. More costs more on every turn that reads one."
          error={problems.chars}
          controlWidth={160}
        >
          <Input value={draft.chars} inputMode="numeric" placeholder={String(l.default_result_chars)} onChange={(e) => set("chars", e.target.value)} />
        </Field>
        </div>
        <div className="apps-pinned">
          <p className="apps-pinned__label">
            Always offered <span className="t-mono t-3 apps-pinned__key">pinned</span>
          </p>
          <p className="t-small t-2">
            When the apps together offer more tools than a turn carries, these stay offered by name and the rest are found by search. Pinning is never a permission.
          </p>
          {tools.length === 0 && gone.length === 0 ? (
            <p className="t-small t-3">Its tools show here once it connects.</p>
          ) : (
            <div className="apps-pinned__list">
              {[...tools, ...gone].map((t) => (
                <Checkbox
                  key={t}
                  label={
                    <>
                      {toolLabel(t)} <span className="t-mono t-3">{t}</span>
                    </>
                  }
                  description={gone.includes(t) ? `${app.title} doesn’t offer this now.` : undefined}
                  checked={draft.pinned.includes(t)}
                  onChange={(on) => set("pinned", on ? [...draft.pinned, t] : draft.pinned.filter((p) => p !== t))}
                />
              ))}
            </div>
          )}
        </div>
      </Card>
    </Section>
  );

  const dock = dirty ? (
    <SaveBar
      variant={saveError ? "error" : invalid ? "error" : online ? "dirty" : "offline"}
      title={saveError ? "Couldn’t save" : invalid ? "Fix a limit" : "Unsaved changes"}
      detail={saveError || invalid ? undefined : `${app.title}’s limits`}
      description={saveError ?? (invalid ? "A limit is out of range." : undefined)}
      busy={busy}
      onSave={() => void save()}
      onDiscard={() => {
        setDraft(saved);
        setSaveError(null);
      }}
    />
  ) : null;

  return { section, dock };
}
