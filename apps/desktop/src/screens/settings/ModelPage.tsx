/**
 * Owner: settings. Settings › Model (ADR 0078): choose your subscription first. The picker of
 * Claude Code, OpenAI Codex and Google Antigravity is the page; the connection made another way
 * (a key, a local model, an address, ADR 0037) is not offered here for now. A Ghost already set
 * up that way keeps it, folded away under "Your connection now" so it can still be changed or
 * tested; a fresh one never sees it.
 */
import type { ModelConnectionView, SettingsView } from "@ghost/client-ts";
import type { ReactNode } from "react";

import { Disclosure, Section } from "../../ui";
import { CLAUDE_CODE } from "./claudeCode";
import { SubscriptionPicker, useSubscriptions } from "./SubscriptionPicker";

/** The provider Ghost uses now when it is none of the subscriptions and is set up: its title. */
export function otherProviderOf(s: Pick<SettingsView, "values">, conn: Pick<ModelConnectionView, "provider" | "ready" | "providers"> | null): string | null {
  if (!conn || conn.provider === CLAUDE_CODE) return null;
  const named = !!(s.values.GHOST_LLM_PROVIDER ?? "").trim();
  if (!conn.ready && !named) return null;
  return conn.providers.find((p) => p.id === conn.provider)?.title ?? conn.provider;
}

export function ModelPage({ s, conn, mock, onUsed, children }: { s: SettingsView; conn: ModelConnectionView | null; mock: boolean; onUsed: () => void; children: ReactNode }) {
  const subs = useSubscriptions();
  const other = otherProviderOf(s, conn);
  return (
    <>
      <Section id="set-subscriptions" className="set-section" title="Your subscription" description="Ghost thinks with the AI subscription you already have, through its own program on this computer.">
        <SubscriptionPicker {...subs} onUsed={onUsed} otherProvider={other} mock={mock} />
      </Section>
      {other && (
        <Section className="set-section">
          <Disclosure summary={`Your connection now: ${other}`}>{children}</Disclosure>
        </Section>
      )}
    </>
  );
}
