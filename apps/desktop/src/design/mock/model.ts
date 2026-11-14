/**
 * The mock daemon's model connection (ADR 0037): the providers ghost_core::connection offers,
 * resolved from the mock's settings the way the daemon resolves them, made-up model lists,
 * a Test connection that answers after a moment, and an OpenRouter sign-in that finishes on
 * its own a few seconds later. Nothing leaves the window.
 */
import { HttpError } from "./errors";
import type { ModelConnectionView, ModelDraft, ModelListView, ModelCheckModel, ModelCheckView, ModelProviderView, SettingsView } from "@ghost/client-ts";

export const MODEL_PROVIDERS: ModelProviderView[] = [
  { id: "anthropic", title: "Anthropic", blurb: "Claude, with a password (API key) from the Anthropic Console.", protocol: "anthropic", base_url: "https://api.anthropic.com", password: "ANTHROPIC_API_KEY", password_optional: false, sign_in: false, local: false, default_model: "claude-sonnet-5-5" },
  { id: "openrouter", title: "OpenRouter", blurb: "Sign in, and Claude or any other model OpenRouter offers.", protocol: "anthropic", base_url: "https://openrouter.ai/api", password: "OPENROUTER_API_KEY", password_optional: false, sign_in: true, local: false, default_model: "anthropic/claude-sonnet-5.5" },
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

  /** The connection a set of values names, as ghost_core::connection resolves it. */
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
      from_environment: [],
    };
  }

  list(draft: ModelDraft): ModelListView {
    const c = this.view(draft);
    const fallback = MODEL_PROVIDERS.find((p) => p.id === c.provider)?.default_model ?? null;
    if (c.provider === "lmstudio") return { models: [], default: fallback, error: "couldn’t reach http://localhost:1234 (ConnectError)" };
    if (c.provider === "custom" && !c.base_url) return { models: [], default: fallback, error: "Give it an address first." };
    if (c.password && !c.password_set && c.provider !== "openrouter" && c.provider !== "custom")
      return { models: [], default: fallback, error: this.vaultOpen() ? "Save its password to see the models it offers." : "Unlock your keyring on Settings › Passwords to see the models it offers." };
    const models = (MODELS[c.provider] ?? []).map((m) => ({ id: m.id, name: m.name ?? null }));
    return { models, default: fallback && models.some((m) => m.id === fallback) ? fallback : models[0]?.id ?? fallback, error: null };
  }

  /** The kept check of the saved connection (ADR 0056). */
  last: ModelCheckView | null = null;

  check(draft: ModelDraft): ModelCheckView {
    const c = this.view(draft);
    const where = `${c.provider} (${c.protocol}, ${c.base_url.replace(/^(https?:\/\/[^/]+).*$/, "$1")})`;
    const at = new Date().toISOString();
    if (!c.ready) return { checked_at: at, where, models: [], advice: c.problems[0] ?? "Not set up." };
    const yes = { state: "yes" as const, detail: null };
    const model = (role: string, id: string, word: number): ModelCheckModel => ({
      role,
      model: id,
      answered_by: id,
      reachable: true,
      error: null,
      first_byte: { median_ms: word - 180, worst_ms: word + 220 },
      first_word: { median_ms: word, worst_ms: word + 420 },
      samples: 3,
      streaming: yes,
      tools: yes,
      caching: c.protocol === "anthropic" ? { state: "yes", detail: "4210 tokens read from the cache" } : { state: "not_reported", detail: "the provider doesn't report it" },
      thinking:
        role === "fast"
          ? { state: "no", detail: "refuses between_tools; Ghost sends its requests without it (HTTP 400)" }
          : c.protocol === "anthropic"
            ? { state: "yes", detail: "accepts between_tools" }
            : { state: "not_applicable", detail: "no thinking setting is sent" },
      thinking_mode: c.protocol === "anthropic" ? "between_tools" : null,
    });
    if (c.provider === "lmstudio") {
      const m = model("main", c.wire_model, 0);
      return { checked_at: at, where, advice: null, models: [{ ...m, reachable: false, error: "I can't reach the model right now. Check the model connection in Settings. (ConnectError: can't reach http://localhost:1234)", answered_by: null }] };
    }
    const word = c.provider === "ollama" ? 238 : c.provider === "openrouter" ? 1840 : 712;
    const models = [model("main", c.wire_model, word)];
    if (c.fast_wire_model) models.push(model("fast", c.fast_wire_model, Math.round(word * 0.6)));
    const slow = word * (c.fast_wire_model ? 0.6 : 1) > 1500;
    const advice = slow
      ? `Spoken replies will start slowly: ${models[models.length - 1].model} took ${((word * (c.fast_wire_model ? 0.6 : 1)) / 1000).toFixed(1)} s to its first word on this connection. Of the connections you've tested, claude-haiku-5-5 via anthropic answered in 0.7 s. Ghost won't change the connection for you: it's in Settings, under Model.`
      : null;
    const r = { checked_at: at, where, models, advice };
    if (!Object.keys(draft.values ?? {}).some((k) => draft.values[k])) this.last = r;
    return r;
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
