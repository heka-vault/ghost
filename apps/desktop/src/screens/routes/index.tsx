/**
 * Owner: routes. How Ghost reaches the operator about what needs them (ADR 0031): which
 * routes are on, how long a delivery waits to be heard before the next one, quiet hours,
 * and what desk voice says first. It edits `[routes]` in the policy file, the operator's
 * own file; the model never sets any of it.
 */
import type { RoutesView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useCallback, useEffect, useMemo, useState } from "react";

import type { RouteProps } from "../../app/routes";
import { Card, Field, Input, LiveStatus, Notice, Page, SaveBar, Section, Select, Skeleton, Switch, errorText, useToast } from "../../ui";
import "./routes.css";

type Name = RoutesView["enabled"][number];

/** Quietest first, as the engine climbs them. */
const ROUTES: { name: Name; title: string; help: string; later?: string }[] = [
  { name: "digest", title: "When you next wake Ghost", help: "Ghost opens with what is waiting. Always on." },
  { name: "desk", title: "Ghost asks at the desk", help: "A chime and your greeting, then it waits for your reply before it says anything else. Silence moves on." },
  { name: "desktop", title: "Desktop notification", help: "On this computer. “Got it” in the notification counts as heard." },
  { name: "phone", title: "The phone app", help: "The list on your phone’s Home, and a notification while the app is open." },
  { name: "call", title: "Ghost calls your phone", help: "The phone rings; answering opens the agent with the question.", later: "Waits for phone push" },
  { name: "sms", title: "Text message", help: "Through a carrier you declare, such as Bird. Carries a count, not titles, unless you say so.", later: "Needs a carrier server" },
  { name: "whatsapp", title: "WhatsApp", help: "Through a carrier you declare, such as Bird.", later: "Needs a carrier server" },
];

const ESCALATE = [5, 10, 15, 30, 60, 120].map((m) => ({ value: String(m), label: m < 60 ? `${m} minutes` : `${m / 60} hour${m > 60 ? "s" : ""}` }));
const QUIET = /^\d{1,2}:\d{2}-\d{1,2}:\d{2}$/;

export default function RoutesScreen(_: RouteProps) {
  const { client, connected } = useGhost();
  const toast = useToast();
  const [saved, setSaved] = useState<RoutesView | null>(null);
  const [draft, setDraft] = useState<RoutesView | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [saveError, setSaveError] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  const load = useCallback(async () => {
    try {
      const p = await client.policy();
      setSaved(p.routes ?? null);
      setDraft(p.routes ?? null);
      setError(null);
    } catch (e) {
      setError(errorText(e));
    }
  }, [client]);
  useEffect(() => {
    void load();
  }, [load]);

  const dirty = useMemo(() => !!draft && JSON.stringify(draft) !== JSON.stringify(saved), [draft, saved]);
  const quietBad = !!draft?.quiet_hours && !QUIET.test(draft.quiet_hours.replace(/\s/g, ""));
  const set = (patch: Partial<RoutesView>) => setDraft((d) => (d ? { ...d, ...patch } : d));
  const toggle = (name: Name, on: boolean) =>
    set({ enabled: ROUTES.map((r) => r.name).filter((n) => (n === name ? on : n === "digest" || draft!.enabled.includes(n))) });

  const save = async () => {
    if (!draft || quietBad) return;
    setBusy(true);
    try {
      const res = await client.updatePolicy({ routes: { ...draft, quiet_hours: draft.quiet_hours?.replace(/\s/g, "") || null } });
      setSaved(res.policy.routes ?? null);
      setDraft(res.policy.routes ?? null);
      setSaveError(null);
      toast({ title: "Saved", tone: "ok" });
    } catch (e) {
      setSaveError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Page
      title="How Ghost reaches you"
      status={<LiveStatus />}
      description="When something needs you, Ghost starts with the quietest way you’ve turned on and moves up only if you haven’t heard it. Saved in your policy file under [routes]."
      dock={
        dirty && (
          <SaveBar
            variant={saveError ? "error" : connected ? "dirty" : "offline"}
            title={saveError ? "Couldn’t save" : "Unsaved changes"}
            description={saveError ?? undefined}
            busy={busy}
            onSave={() => void save()}
            onDiscard={() => {
              setDraft(saved);
              setSaveError(null);
            }}
          />
        )
      }
    >
      {error && (
        <Notice tone="danger" title="Couldn’t load the policy file" className="rt-notice">
          {error}
        </Notice>
      )}
      {!draft && !error ? (
        <Card>
          <Skeleton width="60%" height={14} />
        </Card>
      ) : draft ? (
        <div className="rt-stack">
          <Section title="Ways to reach you" description="Quietest first. Desk ways are used only when you’re at the desk.">
            <Card list>
              {ROUTES.map((r) => {
                const on = r.name === "digest" || draft.enabled.includes(r.name);
                const waiting = !!r.later && !draft.available.includes(r.name);
                return (
                  <div key={r.name} className="rt-row">
                    <Field label={r.title} help={waiting && on ? `${r.help} ${r.later}: listed, but nothing goes out yet.` : r.help} layout="inline" controlWidth="auto">
                      <Switch checked={on} disabled={r.name === "digest"} onChange={(v) => toggle(r.name, v)} aria-label={r.title} />
                    </Field>
                  </div>
                );
              })}
            </Card>
          </Section>

          <Section title="Timing">
            <Card>
              <div className="rt-fields">
                <Field label="Move up after" help="How long a delivery waits for you to hear it before the next way." layout="inline">
                  <Select value={String(draft.escalate_after)} options={ESCALATE} onChange={(e) => set({ escalate_after: Number(e.target.value) })} />
                </Field>
                <Field label="Quiet hours" help="Nothing but the next-wake summary in these hours. Leave empty for none." error={quietBad ? "Write it as 22:00-07:00." : undefined} layout="inline">
                  <Input mono placeholder="22:00-07:00" value={draft.quiet_hours ?? ""} invalid={quietBad} onChange={(e) => set({ quiet_hours: e.target.value || null })} />
                </Field>
                <Field label="What Ghost says first at the desk" help="Before anything else, and nothing about what’s waiting until you reply." layout="inline">
                  <Input value={draft.greeting} maxLength={80} onChange={(e) => set({ greeting: e.target.value })} />
                </Field>
              </div>
            </Card>
          </Section>
          <p className="t-small t-3 rt-foot">
            Say “Ghost, nothing until two” to pause everything but the summary until then. How loud each watch may get is set per watch in its
            department’s folder, or under <span className="t-mono">[routes.urgency]</span> in the policy file.
          </p>
        </div>
      ) : null}
    </Page>
  );
}
