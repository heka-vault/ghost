/**
 * The mock daemon's model connection (ADR 0037): the providers ghost_core.connection offers,
 * resolved from the mock's settings the way the daemon resolves them, made-up model lists,
 * a Test connection that answers after a moment, and an OpenRouter sign-in that finishes on
 * its own a few seconds later. Nothing leaves the window.
 */
import { HttpError } from "./errors";
import type { ModelConnectionView, ModelDraft, ModelListView, ModelProviderView, ModelTestResult, SettingsView } from "@ghost/client-ts";

export const MODEL_PROVIDERS: ModelProviderView[] = [
  { id: "anthropic", title: "Anthropic", blurb: "Claude, with a password (API key) from the Anthropic Console.", protocol: "anthropic", base_url: "https://api.anthropic.com", password: "ANTHROPIC_API_KEY", password_optional: false, sign_in: false, local: false, default_model: "claude-sonnet-5-5" },
  { id: "openrouter", title: "OpenRouter", blurb: "Sign in, and Claude or any other model OpenRouter offers.", protocol: "anthropic", base_url: "https://openrouter.ai/api", password: "OPENROUTER_API_KEY", password_optional: false, sign_in: true, local: false, default_model: "anthropic/claude-sonnet-5.5" },
  { id: "cliproxy", title: "Claude subscription", blurb: "Your Claude plan, through CLIProxy running on your network.", protocol: "anthropic", base_url: "http://localhost:8317", password: "ANTHROPIC_AUTH_TOKEN", password_optional: true, sign_in: false, local: true, default_model: "claude-sonnet-5-5" },
  { id: "openai", title: "OpenAI", blurb: "GPT models, with a password (API key) from the OpenAI platform.", protocol: "openai", base_url: "https://api.openai.com/v1", password: "OPENAI_API_KEY", password_optional: false, sign_in: false, local: false, default_model: "gpt-5" },
  { id: "ollama", title: "Ollama", blurb: "A model running on this computer. Nothing leaves it.", protocol: "openai", base_url: "http://localhost:11434/v1", password: null, password_optional: false, sign_in: false, local: true, default_model: null },
  { id: "lmstudio", title: "LM Studio", blurb: "A model LM Studio serves on this computer. Nothing leaves it.", protocol: "openai", base_url: "http://localhost:1234/v1", password: null, password_optional: false, sign_in: false, local: true, default_model: null },
  { id: "custom", title: "Your own address", blurb: "Any address that speaks Anthropic’s or OpenAI’s API.", protocol: "anthropic", base_url: "", password: "LLM_API_KEY", password_optional: true, sign_in: false, local: false, default_model: null },
];

const MODELS: Record<string, { id: string; name?: string }[]> = {
  anthropic: [
    { id: "claude-haiku-5-5", name: "Claude Haiku 5.5" },
    { id: "claude-opus-5-5", name: "Claude Opus 5.5" },
    { id: "claude-sonnet-5-5", name: "Claude Sonnet 5.5" },
  ],
  openrouter: [
    { id: "anthropic/claude-haiku-5.5", name: "Anthropic: Claude Haiku 5.5" },
    { id: "anthropic/claude-opus-5.5", name: "Anthropic: Claude Opus 5.5" },
    { id: "anthropic/claude-sonnet-5.5", name: "Anthropic: Claude Sonnet 5.5" },
    { id: "google/gemini-3-pro", name: "Google: Gemini 3 Pro" },
    { id: "openai/gpt-5", name: "OpenAI: GPT-5" },
    { id: "openai/gpt-5-mini", name: "OpenAI: GPT-5 mini" },
  ],
  cliproxy: [{ id: "claude-opus-5-5" }, { id: "claude-sonnet-5-5" }],
  openai: [{ id: "gpt-5" }, { id: "gpt-5-mini" }, { id: "gpt-5-nano" }],
  ollama: [{ id: "llama4:8b" }, { id: "qwen3:14b" }],
  lmstudio: [{ id: "qwen3-30b-a3b" }],
  custom: [],
};

const KEYS = ["GHOST_LLM_PROVIDER", "GHOST_LLM_MODEL", "GHOST_LLM_PROTOCOL", "GHOST_LLM_BASE_URL", "GHOST_LLM_PASSWORD", "GHOST_LLM_HEADERS", "GHOST_LLM_TIMEOUT_SECS"];
const toClaudeOnOpenRouter = (m: string) => (m.includes("/") ? m : `anthropic/${m.replace(/^(claude-[a-z]+)-(\d+)-(\d+)$/, "$1-$2.$3")}`);

export class MockModel {
  private signIn: { pending: boolean; error: string | null } = { pending: false, error: null };

  constructor(
    private readonly settings: () => SettingsView,
    /** Whether the vault holds this password (never its value). */
    private readonly held: (name: string) => boolean,
    /** Keep a password in the vault, as OpenRouter's sign-in does. */
    private readonly keep: (name: string) => void,
    private readonly vaultOpen: () => boolean,
  ) {}

  private has(name: string | null | undefined, extra: Record<string, string | null | undefined> = {}) {
    if (!name) return false;
    if (extra[name]) return true;
    return this.held(name);
  }

  /** The connection a set of values names, as ghost_core.connection resolves it. */
  view(draft?: ModelDraft): ModelConnectionView {
    const values: Record<string, string> = { ...this.settings().values };
    for (const [k, v] of Object.entries(draft?.values ?? {})) {
      if (v == null || v === "") delete values[k];
      else values[k] = v;
    }
    let provider = (values.GHOST_LLM_PROVIDER ?? "").trim();
    let legacy: string | null = null;
    if (!provider) {
      if (values.GHOST_LLM_ENDPOINT === "openrouter") {
        provider = "openrouter";
        legacy = "GHOST_LLM_ENDPOINT=openrouter";
      } else provider = "anthropic";
    }
    const spec = MODEL_PROVIDERS.find((p) => p.id === provider) ?? MODEL_PROVIDERS[0];
    const protocol = provider === "custom" ? ((values.GHOST_LLM_PROTOCOL as "anthropic" | "openai") || "anthropic") : spec.protocol;
    const base = (values.GHOST_LLM_BASE_URL || spec.base_url).replace(/\/$/, "");
    const model = values.GHOST_LLM_MODEL ?? "";
    let wire = model || spec.default_model || (protocol === "anthropic" ? "claude-sonnet-5-5" : "");
    if (provider === "openrouter" && wire) wire = toClaudeOnOpenRouter(wire);
    const password = values.GHOST_LLM_PASSWORD || spec.password || null;
    const set = this.has(password, password && draft?.password_value ? { [password]: draft.password_value } : {});
    const problems: string[] = [];
    if (!base) problems.push(`${spec.title} needs an address (GHOST_LLM_BASE_URL)`);
    if (!wire) problems.push("no model is chosen (GHOST_LLM_MODEL)");
    if (password && !spec.password_optional && !set) {
      // While the keyring is locked the key can't be read, which isn't "not set" (as the daemon says).
      if (!this.vaultOpen()) problems.push(`your keyring is locked, so Ghost can't read ${password}. Unlock it on Settings › Passwords`);
      else problems.push(legacy ? `${legacy} but ${password} is not set` : spec.sign_in ? `sign in to ${spec.title} (${password} is not set)` : `${password} is not set`);
    }
    const explicit: Record<string, string> = { GHOST_LLM_PROVIDER: provider };
    for (const k of KEYS.slice(1)) if (values[k]) explicit[k] = values[k];
    return {
      provider,
      protocol,
      base_url: base,
      base_url_override: values.GHOST_LLM_BASE_URL || null,
      model,
      wire_model: wire,
      password,
      password_set: set,
      headers: {},
      timeout_secs: values.GHOST_LLM_TIMEOUT_SECS ? Number(values.GHOST_LLM_TIMEOUT_SECS) : null,
      legacy,
      problems,
      ready: problems.length === 0,
      values: explicit,
      providers: MODEL_PROVIDERS,
      sign_in_pending: this.signIn.pending,
      sign_in_error: this.signIn.error,
    };
  }

  list(draft: ModelDraft): ModelListView {
    const c = this.view(draft);
    const fallback = MODEL_PROVIDERS.find((p) => p.id === c.provider)?.default_model ?? null;
    if (c.provider === "lmstudio") return { models: [], default: fallback, error: "couldn’t reach http://localhost:1234 (ConnectError)" };
    if (c.provider === "custom" && !c.base_url) return { models: [], default: fallback, error: "Give it an address first." };
    if (c.password && !c.password_set && c.provider !== "openrouter" && c.provider !== "custom" && c.provider !== "cliproxy")
      return { models: [], default: fallback, error: this.vaultOpen() ? "Save its password to see the models it offers." : "Unlock your keyring on Settings › Passwords to see the models it offers." };
    const models = (MODELS[c.provider] ?? []).map((m) => ({ id: m.id, name: m.name ?? null }));
    return { models, default: fallback && models.some((m) => m.id === fallback) ? fallback : models[0]?.id ?? fallback, error: null };
  }

  test(draft: ModelDraft): ModelTestResult {
    const c = this.view(draft);
    const where = `${c.provider} (${c.protocol}, ${c.base_url.replace(/^(https?:\/\/[^/]+).*$/, "$1")})`;
    if (!c.ready) return { ok: false, model: c.wire_model, error: c.problems[0] ?? "Not set up.", where, ttft_ms: null, total_ms: null, answered_by: null, reply: null };
    if (c.provider === "lmstudio") return { ok: false, model: c.wire_model, error: "APIConnectionError: couldn’t reach localhost:1234", where, ttft_ms: null, total_ms: null, answered_by: null, reply: null };
    const local = c.provider === "ollama";
    const ttft = local ? 238 : c.provider === "openrouter" ? 1046 : 712;
    return { ok: true, ttft_ms: ttft, total_ms: ttft + 140, model: c.wire_model, answered_by: c.wire_model, reply: "Hello there, good to meet.", error: null, where };
  }

  startSignIn(): { url: string } {
    if (!this.vaultOpen()) throw new HttpError(423, "Ghost’s passwords are locked. Unlock them, then sign in.");
    this.signIn = { pending: true, error: null };
    window.setTimeout(() => {
      this.keep("OPENROUTER_API_KEY");
      this.signIn = { pending: false, error: null };
    }, 3000);
    return { url: "https://openrouter.ai/auth?callback_url=http%3A%2F%2Flocalhost%3A47823%2Fcallback&code_challenge=mock&code_challenge_method=S256&state=mock" };
  }
}

export const isConnectionKey = (k: string) => KEYS.includes(k);
/** A password only the model connection reads: used from the next session, no restart. */
export const isModelOnly = (name: string) => ["ANTHROPIC_AUTH_TOKEN", "ANTHROPIC_API_KEY", "OPENAI_API_KEY", "LLM_API_KEY"].includes(name);
