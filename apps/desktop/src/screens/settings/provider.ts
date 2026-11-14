/**
 * Which provider the model page shows (ADR 0037): the choices it offers, and how a draft of
 * the connection's settings reads. No window, no client: tested on its own.
 */

type Values = Record<string, string>;

/** The four ways in, as the page offers them. OpenAI, Ollama and LM Studio share one. */
export type ProviderChoice = "anthropic" | "openrouter" | "openai-compatible" | "custom";

/** Provider ids Ghost once offered and still reads from settings.env (ADR 0037, amended
 *  2026-11-14): the daemon reports each as the connection it resolves to, and the page never
 *  offers one or writes it back. */
export const LEGACY_PROVIDERS: readonly string[] = ["cliproxy"];

/** The provider these settings name, or "" when none or a retired one (the daemon's view stands). */
export function namedProvider(values: Values): string {
  const p = (values.GHOST_LLM_PROVIDER ?? "").trim();
  return LEGACY_PROVIDERS.includes(p.toLowerCase()) ? "" : p;
}

/** The draft as a connection: explicit once it names a provider, else the settings the daemon
 *  resolved (`reported`, GET /model's `values`) with any edits on top. A retired provider's name
 *  is never carried into it, so a save writes what it resolves to. */
export function draftConnection(values: Values, reported: Values | undefined): { explicit: boolean; values: Values } {
  if (namedProvider(values)) return { explicit: true, values };
  const edits = Object.fromEntries(Object.entries(values).filter(([k, x]) => k !== "GHOST_LLM_PROVIDER" && (x ?? "").trim() !== ""));
  return { explicit: false, values: { ...(reported ?? {}), ...edits } };
}

export const OPENAI_COMPATIBLE = ["openai", "ollama", "lmstudio"] as const;
export const choiceOf = (provider: string): ProviderChoice =>
  (OPENAI_COMPATIBLE as readonly string[]).includes(provider) ? "openai-compatible" : (provider as ProviderChoice);
