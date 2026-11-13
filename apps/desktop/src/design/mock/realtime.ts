/**
 * The mock daemon's realtime voice (ADR 0062): the view resolved from the mock's settings the
 * way the daemon resolves them, the provider list as ghost_core offers it, a spend that grows
 * in the `realtime` scenario, and a check that answers after a moment. Nothing leaves the window.
 */
import type { RealtimeCheckView, RealtimeProviderView, RealtimeView, SettingsView } from "@ghost/client-ts";

export const REALTIME_PROVIDERS: RealtimeProviderView[] = [
  {
    id: "openai",
    title: "OpenAI Realtime",
    models: [
      { id: "gpt-realtime-2.1-mini", title: "GPT Realtime 2.1 mini" },
      { id: "gpt-realtime-2.1", title: "GPT Realtime 2.1" },
    ],
    default_model: "gpt-realtime-2.1-mini",
    voices: ["marin", "cedar", "alloy", "ash", "ballad", "coral", "echo", "sage", "shimmer", "verse"],
    default_voice: "marin",
    password: "OPENAI_API_KEY",
    disclosure: [
      "After the wake, for the whole session, your voice itself goes to OpenAI Realtime: it hears you and answers in its own voice.",
      "Before the wake nothing changes: the wake word and everything Ghost hears while it waits stay on this machine. The phone’s calls, typed turns and scheduled runs keep the voice on this machine.",
      "Ghost’s agent still does every lookup and every change, under your permissions. The realtime voice only talks: it hands your requests to the agent and says its answers word for word, and Ghost checks what it says.",
      "It costs about 1 to 5 US cents a minute of conversation with the mini model and 5 to 15 with the full one, growing as a session gets long, on top of what the model connection already costs, billed by OpenAI Realtime to the key you give it. You can set a daily cap, after which Ghost uses the voice on this machine.",
      "You can switch back to the voice on this machine at any time. Ghost never switches it on by itself, and nothing an app, a tool or a model says can.",
    ],
  },
];

export class MockRealtime {
  last: RealtimeCheckView | null = null;

  constructor(
    private readonly settings: () => SettingsView,
    private readonly held: (name: string) => boolean,
    /** The `realtime` scenario: a day with some use. */
    private readonly used: boolean,
  ) {}

  view(): RealtimeView {
    const v = this.settings().values;
    const p = REALTIME_PROVIDERS[0];
    const password = v.GHOST_REALTIME_PASSWORD || p.password;
    const cap = v.GHOST_REALTIME_DAILY_CAP_USD ? Number(v.GHOST_REALTIME_DAILY_CAP_USD) : null;
    const model = v.GHOST_REALTIME_MODEL || p.default_model;
    return {
      mode: v.GHOST_VOICE_MODE === "realtime" ? "realtime" : "local",
      provider: p.id,
      model,
      voice: v.GHOST_REALTIME_VOICE || p.default_voice,
      password,
      password_set: this.held(password),
      daily_cap_usd: cap,
      providers: REALTIME_PROVIDERS,
      spend: {
        day: new Date().toISOString().slice(0, 10),
        usd: this.used ? 0.37 : 0,
        sessions: this.used ? 6 : 0,
        last_session_usd: this.used ? 0.06 : 0,
        prices_as_of: "2026-11-12",
        priced: p.models.some((m) => m.id === model),
      },
      last_check: this.last,
      problem: null,
    };
  }

  check(): RealtimeCheckView {
    const view = this.view();
    const at = new Date().toISOString();
    const c: RealtimeCheckView = view.password_set
      ? { checked_at: at, provider: view.provider, model: view.model, reachable: true, error: null, connect_ms: 380, first_audio_ms: 540, tools: true, usd: 0.0011 }
      : { checked_at: at, provider: view.provider, model: view.model, reachable: false, error: `the realtime voice's password ${view.password} is not in Ghost's vault`, connect_ms: null, first_audio_ms: null, tools: null, usd: 0 };
    this.last = c;
    return c;
  }
}
