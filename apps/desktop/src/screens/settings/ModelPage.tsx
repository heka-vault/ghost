/**
 * Owner: settings. Settings › Model (ADR 0078): choose your subscription first. The picker of
 * Claude Code, OpenAI Codex and Google Antigravity is the page; the connection made another way
 * (a key, a local model, an address, ADR 0037) is not offered here for now. A Ghost already set
 * up that way keeps it, folded away under "Your connection now" so it can still be changed or
 * tested; a fresh one never sees it.
 */
import type { ModelConnectionView, SettingsResult, SettingsView } from "@ghost/client-ts";
import { useGhost } from "@ghost/ui";
import { useState, type ReactNode } from "react";

import { Card, Disclosure, ListRow, Section, Select, errorText, useLive, useToast } from "../../ui";
import { CLAUDE_CODE } from "./claudeCode";
import { SubscriptionPicker, useSubscriptions } from "./SubscriptionPicker";
import { QUICK_LINE, QUICK_WITH, THINKS_LINE, THINKS_WITH, modelChoices } from "./subscriptions";

/** The provider Ghost uses now when it is none of the subscriptions and is set up: its title. */
export function otherProviderOf(s: Pick<SettingsView, "values">, conn: Pick<ModelConnectionView, "provider" | "ready" | "providers"> | null): string | null {
  if (!conn || conn.provider === CLAUDE_CODE) return null;
  const named = !!(s.values.GHOST_LLM_PROVIDER ?? "").trim();
  if (!conn.ready && !named) return null;
  return conn.providers.find((p) => p.id === conn.provider)?.title ?? conn.provider;
}

export function ModelPage({
  s,
  conn,
  mock,
  onUsed,
  onSaved,
  children,
}: {
  s: SettingsView;
  conn: ModelConnectionView | null;
  mock: boolean;
  onUsed: () => void;
  /** A model chosen here was saved like any setting: the page takes the settings back. */
  onSaved: (r: SettingsResult) => void;
  children: ReactNode;
}) {
  const subs = useSubscriptions();
  const other = otherProviderOf(s, conn);
  return (
    <>
      <Section id="set-subscriptions" className="set-section" title="Your subscription" description="Ghost thinks with the AI subscription you already have, through its own program on this computer.">
        <SubscriptionPicker {...subs} onUsed={onUsed} otherProvider={other} mock={mock} />
        {conn?.provider === CLAUDE_CODE && <GhostModels conn={conn} onSaved={onSaved} />}
      </Section>
      {other && (
        <Section className="set-section">
          <Disclosure summary={`Your connection now: ${other}`}>{children}</Disclosure>
        </Section>
      )}
    </>
  );
}

/**
 * Under the subscription Ghost uses: which of its models Ghost thinks with, and which answers a
 * spoken turn first (ADR 0050). Saved like any setting (`PUT /settings`, settings.env), so a
 * session picks it up from its next start.
 */
export function GhostModels({ conn, onSaved }: { conn: ModelConnectionView; onSaved: (r: SettingsResult) => void }) {
  const { client } = useGhost();
  const { online } = useLive();
  const toast = useToast();
  const [busy, setBusy] = useState<string | null>(null);
  const c = modelChoices(conn);
  const save = async (key: "GHOST_LLM_MODEL" | "GHOST_LLM_FAST_MODEL", value: string) => {
    setBusy(key);
    try {
      const r = await client.updateSettings({ values: { [key]: value || null } });
      onSaved(r);
      toast({ title: "Saved", description: r.restart_required ? "Ghost picks it up at its next start." : "Ghost uses it from its next session.", tone: "ok" });
    } catch (e) {
      toast({ title: "Couldn’t change the model", description: errorText(e), tone: "danger" });
    } finally {
      setBusy(null);
    }
  };
  return (
    <Card list>
      <ListRow
        title={THINKS_WITH}
        description={THINKS_LINE}
        trailing={
          <Select size="sm" aria-label={THINKS_WITH} value={c.thinks.value} options={c.thinks.options} disabled={!online || !!busy} onChange={(e) => void save("GHOST_LLM_MODEL", e.target.value)} />
        }
      />
      <ListRow
        title={QUICK_WITH}
        description={c.quick.note ? `${QUICK_LINE} ${c.quick.note}` : QUICK_LINE}
        trailing={
          <Select
            size="sm"
            aria-label={QUICK_WITH}
            value={c.quick.value}
            options={c.quick.options}
            disabled={!online || !!busy || c.quick.disabled}
            onChange={(e) => void save("GHOST_LLM_FAST_MODEL", e.target.value)}
          />
        }
      />
    </Card>
  );
}
