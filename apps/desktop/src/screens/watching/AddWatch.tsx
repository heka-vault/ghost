/**
 * Owner: watching. Adding a watch without knowing its file: plain choices (which app, what to
 * look for, how often, how loud), sent to Ghost as the operator's own typed words. Ghost writes
 * it with its one validated writer (`ghost_watch_manage`, ADR 0052), which asks for the
 * operator's okay by default, so a watch is never written around the gate or from this window.
 */
import type { AppView, ConversationView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useEffect, useState } from "react";

import { navigate } from "../../app/router";
import { Button, Dialog, Field, Select, SegmentedControl, Textarea, agentName, errorText, useLive, useToast } from "../../ui";

export type Often = "5m" | "15m" | "1h" | "1d";
export type Loud = "digest" | "notify" | "call";

const OFTEN: { value: Often; label: string; words: string }[] = [
  { value: "5m", label: "5 min", words: "every 5 minutes" },
  { value: "15m", label: "15 min", words: "every 15 minutes" },
  { value: "1h", label: "Hourly", words: "every hour" },
  { value: "1d", label: "Daily", words: "once a day" },
];
const LOUD: { value: Loud; label: string; words: string }[] = [
  { value: "digest", label: "In the summary", words: "in the summary" },
  { value: "notify", label: "Notification", words: "with a notification" },
  { value: "call", label: "Call", words: "with a call" },
];

export interface WatchAsk {
  app: string;
  appTitle: string;
  what: string;
  often: Often;
  loud: Loud;
  /** An agent to act on each new item, and what it should do; none just tells. */
  agent: string;
  agentTitle: string;
  act: string;
}

/** The request in the operator's words, as Ghost will read it. Pure and tested. */
export function watchRequest(a: WatchAsk): string {
  const often = OFTEN.find((o) => o.value === a.often)?.words ?? "every hour";
  const loud = LOUD.find((o) => o.value === a.loud)?.words ?? "with a notification";
  const base = `Set up a watch on ${a.appTitle} (${a.app}): ${a.what.trim().replace(/[.\s]+$/, "")}. Look ${often}, and tell me about each new one ${loud}.`;
  return a.agent && a.act.trim() ? `${base} For each new one, have ${a.agentTitle} ${a.act.trim().replace(/[.\s]+$/, "")}.` : base;
}

export function AddWatchDialog({ open, onClose, agents }: { open: boolean; onClose: () => void; agents: ConversationView[] }) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [apps, setApps] = useState<AppView[]>([]);
  const [d, setD] = useState<WatchAsk>({ app: "", appTitle: "", what: "", often: "15m", loud: "notify", agent: "", agentTitle: "", act: "" });
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    if (!open) return;
    setError(null);
    client
      .apps()
      .then((a) => setApps(a.filter((x) => x.state === "connected" && x.actions.some((t) => t.reads))))
      .catch(() => setApps([]));
  }, [open, client]);

  const ready = !!d.app && d.what.trim().length > 2 && (!d.agent || d.act.trim().length > 2);
  const send = async () => {
    if (!ready || busy) return;
    setBusy(true);
    setError(null);
    try {
      await client.say(watchRequest(d));
      toast({ title: "Asked Ghost", description: "It writes the watch once you say yes, and it shows here." });
      onClose();
      navigate("/session");
    } catch (e) {
      setError(errorText(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      dismissible={!busy}
      size="md"
      title="Add a watch"
      description="Ghost reads one of an app’s lists on its own and tells you what’s new. It sets the watch up and asks for your okay first."
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancel
          </Button>
          <Button variant="primary" type="submit" form="wat-watch-form" loading={busy} disabled={!ready || !online}>
            Ask Ghost to set it up
          </Button>
        </>
      }
    >
      <form
        id="wat-watch-form"
        className="wat-form"
        onSubmit={(e) => {
          e.preventDefault();
          void send();
        }}
      >
        <Field label="In which app" error={apps.length === 0 ? "Connect an app with something to read first, on Apps." : undefined}>
          <Select
            value={d.app}
            onChange={(e) => {
              const a = apps.find((x) => x.name === e.target.value);
              setD((x) => ({ ...x, app: e.target.value, appTitle: a?.title ?? e.target.value }));
            }}
            options={[{ value: "", label: "Pick an app" }, ...apps.map((a) => ({ value: a.name, label: a.title }))]}
          />
        </Field>
        <Field label="What to look for" help="In your words. Ghost picks the app’s read and the fields to keep.">
          <Textarea rows={2} value={d.what} placeholder="New issues labeled bug in ghost-mock/app" onChange={(e) => setD((x) => ({ ...x, what: e.target.value }))} />
        </Field>
        <Field label="How often">
          <SegmentedControl aria-label="How often" value={d.often} onChange={(v) => setD((x) => ({ ...x, often: v }))} options={OFTEN} />
        </Field>
        <Field label="How it tells you" help="Your quiet hours still hold.">
          <SegmentedControl aria-label="How it tells you" value={d.loud} onChange={(v) => setD((x) => ({ ...x, loud: v }))} options={LOUD} />
        </Field>
        <Field label="And then" optional help="An agent can act on each new one, at most twelve times an hour, under your permissions.">
          <Select
            value={d.agent}
            onChange={(e) => setD((x) => ({ ...x, agent: e.target.value, agentTitle: agentName(e.target.value, agents.find((a) => a.slug === e.target.value)?.name) }))}
            options={[{ value: "", label: "Just tell me" }, ...agents.filter((a) => !a.is_ghost).map((a) => ({ value: a.slug, label: `Have ${agentName(a.slug, a.name)} act on it` }))]}
          />
        </Field>
        {d.agent && (
          <Field label={`What ${d.agentTitle} does`}>
            <Textarea rows={2} value={d.act} placeholder="label it, and draft a first reply" onChange={(e) => setD((x) => ({ ...x, act: e.target.value }))} />
          </Field>
        )}
      </form>
    </Dialog>
  );
}
