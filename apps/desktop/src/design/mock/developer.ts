/**
 * The mock daemon's Settings › Developer (ADR 0072, ADR 0073): the experimental switches,
 * debug mode with its hour, and the diagnostic report's preview and save. Shaped as
 * `ghost_api::developer` answers; nothing is written anywhere and no number is measured.
 * `?debug=on` (or the `debugmode` scenario) starts with debug mode on, 42 minutes left.
 */
import type { DebugAudioMark, DebugAudioView, DiagnosePart, DiagnosePreview, DiagnoseSaved, SwitchView, SwitchesView } from "@ghost/client-ts";

const sw = (name: string, title: string, sentence: string, kind: SwitchView["kind"], dflt: string, extra: Partial<SwitchView> = {}): SwitchView => ({
  name,
  title,
  sentence,
  kind,
  value: "",
  file_value: null,
  source: "default",
  default: dflt,
  choices: [],
  max: kind === "number" ? 5 : null,
  ...extra,
});

function switches(): SwitchView[] {
  return [
    sw("GHOST_TTS_STREAM", "Windowed voice", "Kokoro renders a piece whole, or a window of frames at a time and plays each as it is made (ADR 0053).", "choice", "whole", { choices: ["whole", "windowed"], value: "windowed", file_value: "windowed", source: "file" }),
    sw("GHOST_STT_EARLY_READ", "Speculative answers", "Reads a segment's words at its first quiet; speculate starts the held answer from them while the stop's read still makes the transcript (ADR 0054).", "choice", "off", { choices: ["off", "verify", "speculate"] }),
    sw("GHOST_LLM_RACE", "Race the main model", "With a fast model set, asks the main model beside every fast call and uses its reply when the fast model hands over (ADR 0062).", "flag", "on"),
    sw("GHOST_FIRST_SOUND_SECS", "Wait before “One sec.”", "Seconds after a turn's reply begins before Ghost says “One sec.” when the model has written nothing; unset chooses it from the connection's measured first word, 0 never says it.", "number", "", { max: 10 }),
    sw("GHOST_TURN_GRACE_SECS", "Turn grace", "Seconds Ghost waits after you stop before it takes your turn as finished.", "number", "1.2", { value: "0.9", source: "environment" }),
    sw("GHOST_BARGE_IN_MIN_WORDS", "Words to cut Ghost off", "How many words of yours stop Ghost while it speaks.", "number", "2", { max: 10 }),
    sw("GHOST_ACK", "Acknowledgments", "Ghost says a short acknowledgment before a slow call.", "flag", "on"),
    sw("GHOST_LATENCY", "Timing records", "Ghost keeps each turn's timings (numbers only) in latency.jsonl, for ghostd latency and the diagnostic report.", "flag", "on"),
  ];
}

const PARTS: DiagnosePart[] = [
  { key: "about", title: "This Ghost and this computer", summary: "version, build, how it was installed, the OS and the session type", personal: false, bytes: 612 },
  { key: "sound", title: "Sound", summary: "the microphone and speakers in use, how Ghost hears you, the echo canceller", personal: false, bytes: 1_380 },
  { key: "model", title: "Model", summary: "the provider and model names and the last check's timings; never a key or an address", personal: false, bytes: 904 },
  { key: "speech_models", title: "Speech models", summary: "whether Parakeet and Kokoro are here", personal: false, bytes: 180 },
  { key: "latency", title: "Turn timings", summary: "30 recent turns, numbers only", personal: false, bytes: 21_400 },
  { key: "wake_attempts", title: "Wake attempts", summary: "counts and scores only, no sound and no words", personal: false, bytes: 3_100 },
  { key: "error_reports", title: "Error reports", summary: "2 kept, exactly as they were sent", personal: false, bytes: 4_820 },
  { key: "counts", title: "Apps, agents and schedules", summary: "how many, and the apps' names and states; never what they hold", personal: false, bytes: 260 },
  { key: "log", title: "Ghost's log", summary: "the last 400 lines, Ghost's own words only, scrubbed", personal: false, bytes: 38_900 },
];
const ADDED: Record<string, DiagnosePart> = {
  transcripts: { key: "transcripts", title: "Recent transcripts", summary: "40 turns of your conversation with Ghost, word for word", personal: true, bytes: 9_800 },
  debug_log: { key: "debug_log", title: "Debug log", summary: "312 lines of what Ghost heard and decided while debug mode was on", personal: true, bytes: 41_200 },
  wake_recordings: { key: "wake_recordings", title: "Wake recordings", summary: "8 recordings of your voice saying Ghost's name", personal: true, bytes: 3_640_000 },
};

export class MockDeveloper {
  private until: number | null;
  private list = switches();
  private timer: number | null = null;

  constructor(
    debugOn: boolean,
    private onChange: () => void,
  ) {
    this.until = debugOn ? Date.now() + 42 * 60_000 + 17_000 : null;
  }

  mark(): DebugAudioMark | null {
    return this.until && this.until > Date.now() ? { until: new Date(this.until).toISOString() } : null;
  }

  view(): DebugAudioView {
    const m = this.mark();
    return { on: !!m, until: m?.until ?? null, from_environment: false, log_bytes: m ? 41_200 : 0 };
  }

  private switches(): SwitchesView {
    return { switches: this.list, settings_path: "~/.config/ghost/settings.env" };
  }

  handle(method: string, path: string, b: Record<string, unknown>): unknown {
    if (path === "/debug-audio" && method === "GET") return this.view();
    if (path === "/debug-audio" && method === "PUT") {
      this.until = b.on ? Date.now() + 60 * 60_000 : null;
      if (this.timer) window.clearTimeout(this.timer);
      if (this.until) this.timer = window.setTimeout(() => ((this.until = null), this.onChange()), 60 * 60_000);
      this.onChange();
      return this.view();
    }
    if (path === "/developer/switches" && method === "GET") return this.switches();
    if (path === "/developer/switches" && method === "PUT") {
      const values = (b.values ?? {}) as Record<string, string | null>;
      this.list = this.list.map((s) => (s.name in values ? { ...s, file_value: values[s.name] || null, value: values[s.name] || "", source: values[s.name] ? "file" : "default" } : s));
      return { switches: this.switches(), applied: [], restart_required: true };
    }
    if (path === "/diagnose/preview" && method === "POST") {
      const parts = [...PARTS, ...(["transcripts", "debug_log", "wake_recordings"] as const).filter((k) => b[k]).map((k) => ADDED[k])];
      const p: DiagnosePreview = {
        parts,
        bytes: parts.reduce((n, x) => n + x.bytes, 0) + 900,
        path: "~/Downloads/ghost-diagnostic-20261114-153012.json",
        debug_log_available: !!this.mark(),
        wake_recordings: 8,
      };
      return p;
    }
    if (path === "/diagnose" && method === "POST") {
      const s: DiagnoseSaved = { path: "~/Downloads/ghost-diagnostic-20261114-153012.json", bytes: 72_416 };
      return s;
    }
    return undefined;
  }
}
