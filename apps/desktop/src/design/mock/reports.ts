/**
 * The mock daemon's error reports (ADR 0067, `ghostd::reports`): a released build's view with
 * the switch on and two reports this install sent, shaped exactly as `ghost_reports` builds
 * them. The ids are made up; nothing here is sent anywhere.
 */
import type { KeptReportView, ReportsView } from "@ghost/client-ts";

const KEY = "(Ghost's project key)";
const INSTALL = "5b0f3c1e-8d2a-4e57-9a61-2c7d40e8b913";

function body(at: string, kind: string, component: string, type: string, value: string, extra: Record<string, unknown>, frames: unknown[] | null): KeptReportView["body"] {
  const entry: Record<string, unknown> = { type, value, mechanism: { handled: kind !== "panic", synthetic: false, type: "generic" } };
  if (frames) entry.stacktrace = { type: "raw", frames };
  return {
    api_key: KEY,
    event: "$exception",
    distinct_id: INSTALL,
    timestamp: at,
    properties: {
      $exception_list: [entry],
      $exception_fingerprint: "9c41d07a2be35f18",
      $process_person_profile: false,
      $geoip_disable: true,
      $lib: "ghost-reports",
      $lib_version: "2026.11.1",
      ghost_kind: kind,
      ghost_version: "2026.11.1",
      ghost_component: component,
      ghost_facts: extra,
      os_name: "Ubuntu",
      os_version: "24.04",
      session_type: "wayland",
      arch: "x86_64",
      installed_as: "deb",
    },
  };
}

export class MockReports {
  private send = true;
  private kept: KeptReportView[];

  constructor() {
    const a = new Date(Date.now() - 26 * 60_000).toISOString();
    const b = new Date(Date.now() - 3 * 3600_000).toISOString();
    this.kept = [
      {
        sent_at: a,
        kind: "failure",
        component: "daemon",
        title: "output_unavailable: Ghost’s sound output stopped playing",
        body: body(a, "failure", "daemon", "output_unavailable", "Ghost's sound output stopped playing", {}, null),
      },
      {
        sent_at: b,
        kind: "panic",
        component: "daemon",
        title: "panic: index out of bounds",
        body: body(b, "panic", "daemon", "panic", "index out of bounds", { location: "crates/ghost-voice/src/tts.rs:412:27", thread: "tokio-runtime-worker" }, [
          { platform: "custom", lang: "rust", function: "ghostd::session::Session::run::{{closure}}", filename: "crates/ghostd/src/session.rs", lineno: 288, colno: 14, in_app: true, resolved: true },
          { platform: "custom", lang: "rust", function: "ghost_voice::tts::Tts::clause", filename: "crates/ghost-voice/src/tts.rs", lineno: 412, colno: 27, in_app: true, resolved: true },
        ]),
      },
    ];
  }

  view(): ReportsView {
    return { available: true, send: this.send, install_id: this.send ? INSTALL : null, pending: 0, kept: this.kept };
  }

  handle(method: string, path: string, b: Record<string, unknown>): unknown {
    if (method === "GET" && path === "/reports") return this.view();
    if (method === "PUT" && path === "/reports/choice") {
      this.send = !!b.send;
      return this.view();
    }
    return undefined;
  }
}
