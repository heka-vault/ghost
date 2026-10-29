/** Owner: settings. Settings, the passwords (names and metadata only, ADR 0036), audio devices, models. */
import type { DevicesView, ModelsView, PasswordInfo, PasswordsView, PhoneAddress, PhoneView, SettingsView } from "@ghost/client-ts";

import { ago, type Scenario } from "../scenarios";

export interface SettingsFixture {
  settings: SettingsView;
  devices: DevicesView;
  models: ModelsView;
  /** The phone listener and the paired phone (ADR 0016). */
  phone: PhoneView;
  /** Ghost's vault (ADR 0036): what GET /passwords answers. */
  passwords: PasswordsView;
}

/** The LAN address the phone listener binds in the mock (GHOST_PHONE_LISTEN). */
export const PHONE_ADDRESS = "192.168.1.20:8765";
/** The tunnel address, for ?phone=tunnel and tunnel-down (ADR 0020). */
export const PHONE_TUNNEL_ADDRESS = "10.8.0.4:8765";

/**
 * Off before setup, on with nothing paired when new, a phone paired after months of use. The
 * home address only, unless the state adds the tunnel; with a tunnel that is up, the phone
 * last came in through it.
 */
function phone(s: Scenario): PhoneView {
  const state = s.phone ?? (!s.configured ? "off" : s.data === "busy" ? "paired" : "unpaired");
  if (state === "off") return { address: null, addresses: [], listen: [], device: null };
  const home: PhoneAddress = { address: PHONE_ADDRESS, network: "home", listening: true };
  const listen: PhoneAddress[] =
    state === "tunnel" ? [home, { address: PHONE_TUNNEL_ADDRESS, network: "tunnel", listening: true }]
    : state === "tunnel-down" ? [home, { address: PHONE_TUNNEL_ADDRESS, network: null, listening: false }]
    : [home];
  const view = { address: PHONE_ADDRESS, addresses: listen.map((a) => a.address), listen };
  if (state === "unpaired") return { ...view, device: null };
  const away = state === "tunnel";
  return {
    ...view,
    device: {
      id: "d7f3a91c2e4b",
      name: "Pixel 9",
      paired_at: ago(60 * 24 * 12),
      last_seen: s.origin === "phone" ? ago(0.1) : away ? ago(3) : ago(95),
      last_address: away ? PHONE_TUNNEL_ADDRESS : PHONE_ADDRESS,
      last_network: away ? "tunnel" : "home",
    },
  };
}

const SECRET_NAMES = ["ANTHROPIC_AUTH_TOKEN", "GITHUB_TOKEN", "HF_TOKEN", "OPENROUTER_API_KEY", "TYPESAFE_API_KEY", "ELEVENLABS_API_KEY"];
/** Keys only one model provider uses (ghost_api::passwords::PROVIDER_ONLY): the mock daemon lists
 *  each once it is set or once its provider is the one in use, as GET /passwords does. */
export const PROVIDER_ONLY = ["ANTHROPIC_API_KEY", "OPENAI_API_KEY", "LLM_API_KEY"];

/** What Ghost itself reads each of its own passwords for (ghost_core::passwords::GHOST_USES). */
export const GHOST_USES: Record<string, string> = {
  ANTHROPIC_AUTH_TOKEN: "The model connection",
  ANTHROPIC_API_KEY: "The model connection",
  OPENAI_API_KEY: "The model connection",
  LLM_API_KEY: "The model connection",
  OPENROUTER_API_KEY: "Jev, and the model when it goes through OpenRouter",
  TYPESAFE_API_KEY: "Jev",
  ELEVENLABS_API_KEY: "The ElevenLabs voice",
  HF_TOKEN: "Voice model downloads",
  GITHUB_TOKEN: "An app you added by hand that names ${GITHUB_TOKEN}",
};

/** The app each kept key belongs to, by its title in fixtures/apps.ts. */
const APP_TITLES: Record<string, string> = {
  GHOST_APP_MOBBIN_KEY: "Mobbin",
  GHOST_APP_GITHUB_ACME_KEY: "GitHub (Acme)",
  GHOST_APP_SENTRY_SENTRY_ACCESS_TOKEN: "Sentry",
};

function passwords(s: Scenario, set: Set<string>, apps: string[]): PasswordsView {
  const used: Record<string, number> = {
    ANTHROPIC_AUTH_TOKEN: 3,
    OPENROUTER_API_KEY: 3,
    GHOST_APP_MOBBIN_KEY: 60 * 26,
    GHOST_APP_GITHUB_ACME_KEY: 12,
    GHOST_APP_SENTRY_SENTRY_ACCESS_TOKEN: 60 * 24 * 9,
  };
  const row = (name: string, extra: Partial<PasswordInfo> = {}): PasswordInfo => {
    const on = set.has(name);
    const ghost = name in GHOST_USES && name !== "GITHUB_TOKEN";
    return {
      name,
      set: on,
      ghost,
      used_by: ghost ? [GHOST_USES[name]] : APP_TITLES[name] ? [APP_TITLES[name]] : [],
      added: on ? ago(60 * 24 * 40) : null,
      updated: on ? ago(60 * 24 * (name === "ANTHROPIC_AUTH_TOKEN" ? 6 : 40)) : null,
      last_used: on && used[name] ? ago(used[name]) : null,
      ...extra,
    };
  };
  const rows = [SECRET_NAMES[0], ...PROVIDER_ONLY, ...SECRET_NAMES.slice(1), ...apps].map((n) => row(n));
  // Busy: a webhook's password the operator added by hand, and one a hand-written app needs.
  if (s.data === "busy") {
    rows.push(row("JOBS_HOOK_SECRET", { set: true, used_by: ["Webhook jobs"], added: ago(60 * 24 * 20), updated: ago(60 * 24 * 20), last_used: ago(45) }));
    rows.push(row("LINEAR_TOKEN", { set: false, used_by: ["Linear (by hand)"] }));
  }
  return {
    vault: { state: "open", problem: null, path: "~/.local/share/ghost/passwords.vault" },
    passwords: rows,
    // Months of use, from before the vault: the plaintext files still wait to be imported.
    importable:
      s.data === "busy"
        ? [
            { kind: "heka_vault", path: "~/.config/heka-vault/ghost.env" },
            { kind: "file", path: "~/.config/ghost/env" },
            { kind: "file", path: "~/.config/ghost/apps.env" },
          ]
        : [],
  };
}
/** Keys Ghost keeps for the apps it connected (ADR 0034), as fixtures/apps.ts names them. */
const APP_SECRETS: Record<string, string[]> = {
  empty: [],
  normal: ["GHOST_APP_MOBBIN_KEY"],
  busy: ["GHOST_APP_GITHUB_ACME_KEY", "GHOST_APP_MOBBIN_KEY", "GHOST_APP_SENTRY_SENTRY_ACCESS_TOKEN"],
};

export default function settings(s: Scenario): SettingsFixture {
  const configured = s.configured;
  const apps = APP_SECRETS[s.data] ?? [];
  const set = new Set(configured ? ["ANTHROPIC_AUTH_TOKEN", "OPENROUTER_API_KEY", ...apps] : []);
  const values: Record<string, string> = configured
    ? {
        GHOST_INPUT_DEVICE: "ghost.mic",
        GHOST_WAKE_PHRASES: s.data === "busy" ? "hey ghost,okay ghost,ghost wake up" : "hey ghost,okay ghost",
        GHOST_WAKE_FUZZY: "0.8",
        GHOST_JEV: "auto",
        // The model connection (ADR 0037): busy names its provider, CLIProxy on the LAN; normal
        // still has the older setting, OpenRouter by GHOST_LLM_ENDPOINT, as the operator did.
        ...(s.data === "busy"
          ? { GHOST_LLM_PROVIDER: "cliproxy", GHOST_LLM_BASE_URL: "http://192.168.1.5:8317", GHOST_LLM_MODEL: "claude-sonnet-5-5" }
          : { GHOST_LLM_ENDPOINT: "openrouter" }),
      }
    : // First run: nothing set yet, the default microphone.
      {};
  return {
    settings: {
      settings_path: "~/.config/ghost/settings.env",
      values,
      secrets: [...SECRET_NAMES, ...apps].map((name) => ({ name, set: set.has(name) })),
    },
    passwords: passwords(s, set, apps),
    devices: {
      echo_cancel_active: configured,
      echo_cancel_source_present: configured,
      // Busy: the microphone was changed to ghost.mic and saved, but Ghost hasn't restarted since,
      // so it still listens on the USB microphone. Settings shows the difference.
      input_device: !configured ? null : s.data === "busy" ? "Shure MV7+: USB Audio (hw:2,0)" : "ghost.mic",
      output_device: s.data === "busy" ? "Shure MV7+: USB Audio (hw:2,0)" : null,
      devices: [
        { index: 0, name: "HDA Intel PCH: ALC1220 Analog (hw:0,0)", max_input_channels: 2, max_output_channels: 0, default_samplerate: 44100, is_default_input: false, is_default_output: false },
        { index: 2, name: "HDA Intel PCH: HDMI 0 (hw:0,3)", max_input_channels: 0, max_output_channels: 8, default_samplerate: 44100, is_default_input: false, is_default_output: false },
        { index: 7, name: "Shure MV7+: USB Audio (hw:2,0)", max_input_channels: 1, max_output_channels: 2, default_samplerate: 48000, is_default_input: false, is_default_output: false },
        { index: 11, name: "pipewire", max_input_channels: 128, max_output_channels: 128, default_samplerate: 44100, is_default_input: false, is_default_output: false },
        { index: 12, name: "pulse", max_input_channels: 32, max_output_channels: 32, default_samplerate: 44100, is_default_input: false, is_default_output: false },
        { index: 16, name: "default", max_input_channels: 128, max_output_channels: 128, default_samplerate: 44100, is_default_input: true, is_default_output: true },
      ],
    },
    models: {
      ready: s.modelsReady,
      cache_dir: "~/.cache/ghost",
      kokoro_present: s.modelsReady || s.phase !== "starting",
      stt_model: "nemo-parakeet-tdt-0.6b-v3",
    },
    phone: phone(s),
  };
}
