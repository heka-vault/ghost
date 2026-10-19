/** Owner: settings. Settings and secrets (set / not set only), audio devices, models. */
import type { DevicesView, ModelsView, PhoneView, SettingsView } from "@ghost/client-ts";

import { ago, type Scenario } from "../scenarios";

export interface SettingsFixture {
  settings: SettingsView;
  devices: DevicesView;
  models: ModelsView;
  /** The phone listener and the paired phone (ADR 0016). */
  phone: PhoneView;
}

/** The LAN address the phone listener binds in the mock (GHOST_PHONE_LISTEN). */
export const PHONE_ADDRESS = "192.168.1.20:8765";

/** Off before setup, on with nothing paired when new, a phone paired after months of use. */
function phone(s: Scenario): PhoneView {
  const state = s.phone ?? (!s.configured ? "off" : s.data === "busy" ? "paired" : "unpaired");
  if (state === "off") return { address: null, device: null };
  if (state === "unpaired") return { address: PHONE_ADDRESS, device: null };
  return {
    address: PHONE_ADDRESS,
    device: { id: "d7f3a91c2e4b", name: "Pixel 9", paired_at: ago(60 * 24 * 12), last_seen: s.origin === "phone" ? ago(0.1) : ago(95) },
  };
}

const SECRET_NAMES = ["ANTHROPIC_AUTH_TOKEN", "GITHUB_TOKEN", "HF_TOKEN", "OPENROUTER_API_KEY", "TYPESAFE_API_KEY"];

export default function settings(s: Scenario): SettingsFixture {
  const configured = s.configured;
  const set = new Set(configured ? ["ANTHROPIC_AUTH_TOKEN", "GITHUB_TOKEN", "OPENROUTER_API_KEY"] : []);
  const values: Record<string, string> = configured
    ? {
        GHOST_REPOS: "ghost-mock/app,ghost-mock/infra",
        GHOST_INPUT_DEVICE: "ghost.mic",
        GHOST_WAKE_PHRASES: "hey ghost,okay ghost",
        GHOST_WAKE_FUZZY: "0.8",
        GHOST_JEV: "auto",
        // Threads (ADR 0019): both crossing-streams settings off by default; no per-owner tokens yet.
        GHOST_CROSS_DEPARTMENT_ANSWERS: "0",
        GHOST_DEPARTMENT_ROUTING: "0",
        ...(s.data === "busy"
          ? {
              GHOST_LLM_MODEL: "claude-sonnet-5-5",
              GHOST_REPOS: "ghost-mock/app,ghost-mock/infra,ghost-mock/docs,heka-vault/ghost",
              GHOST_WAKE_PHRASES: "hey ghost,okay ghost,ghost wake up",
              // Months in: scoped lines answer read-only across streams, and two organisations have their own tokens.
              GHOST_CROSS_DEPARTMENT_ANSWERS: "1",
              GHOST_GITHUB_TOKENS: "ghost-mock=GHOST_MOCK_TOKEN,heka-vault=HEKA_ORG_TOKEN",
            }
          : {}),
      }
    : // First run: practice GitHub on, nothing allowed yet, the default microphone.
      { GHOST_GITHUB_MOCK: "1" };
  return {
    settings: {
      settings_path: "~/.config/ghost/settings.env",
      env_path: "~/.config/ghost/env",
      values,
      secrets: SECRET_NAMES.map((name) => ({ name, set: set.has(name) })),
      vault_managed: configured && s.data === "busy",
    },
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
