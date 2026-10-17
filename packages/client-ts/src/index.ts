/**
 * Typed client for the Ghost control API (ADR 0013).
 *
 * The daemon listens on a Unix socket, which a browser or a WebView cannot open directly,
 * so this package is transport-agnostic: give it a `fetch` and a way to open the event
 * stream. Two transports ship with it:
 *
 *  - `httpTransport(baseUrl)`: plain fetch + WebSocket. Used in development, where Vite's
 *    dev server proxies `/api` and `/api/events` to the socket.
 *  - `tauriTransport()` (in apps/desktop): Tauri commands that talk to the socket from Rust.
 *
 * Everything typed here comes from `schema.d.ts`, generated from packages/api/openapi.json.
 * Never hand-edit the schema; run `pnpm gen`.
 */
import createClient, { type Client } from "openapi-fetch";

import type { components, paths } from "./schema";

export type Schemas = components["schemas"];
export type Status = Schemas["Status"];
export type Phase = Schemas["Phase"];
export type Turn = Schemas["Turn"];
export type PendingProposal = Schemas["PendingProposal"];
export type OutcomeView = Schemas["OutcomeView"];
export type AuditPage = Schemas["AuditPage"];
export type AuditEntry = Schemas["AuditEntry"];
export type AbilityInfo = Schemas["AbilityInfo"];
export type OperationInfo = Schemas["OperationInfo"];
export type PolicyView = Schemas["PolicyView"];
export type PolicyUpdate = Schemas["PolicyUpdate"];
export type PolicyResult = Schemas["PolicyResult"];
export type SettingsView = Schemas["SettingsView"];
export type SettingsUpdate = Schemas["SettingsUpdate"];
export type DevicesView = Schemas["DevicesView"];
export type ModelsView = Schemas["ModelsView"];
export type Ack = Schemas["Ack"];

/** Payloads on the /events WebSocket (first message is a plain Status snapshot). */
export type GhostEvent =
  | Schemas["StatusEvent"]
  | Schemas["TurnEvent"]
  | Schemas["PendingEvent"]
  | Schemas["OutcomeEvent"]
  | Schemas["LogEvent"];

export type Unsubscribe = () => void;

export interface EventStream {
  /** Receive the initial Status, then every event. `onClose` fires on disconnect. */
  subscribe(onEvent: (e: GhostEvent | Status) => void, onClose?: () => void): Unsubscribe;
}

export interface Transport {
  fetch: typeof fetch;
  baseUrl: string;
  events: EventStream;
}

export const AUTONOMY_LEVELS = ["deny", "confirm", "announce", "quiet"] as const;
export type Autonomy = (typeof AUTONOMY_LEVELS)[number];

/** Narrow an incoming message: the first WebSocket message is a Status, the rest are events. */
export function isEvent(m: GhostEvent | Status): m is GhostEvent {
  return typeof (m as GhostEvent).type === "string";
}

export class GhostClient {
  readonly api: Client<paths>;
  readonly events: EventStream;

  constructor(transport: Transport) {
    this.api = createClient<paths>({ baseUrl: transport.baseUrl, fetch: transport.fetch });
    this.events = transport.events;
  }

  // --- session -----------------------------------------------------------------
  status() {
    return unwrap(this.api.GET("/status"));
  }
  wake(text?: string) {
    return unwrap(this.api.POST("/wake", { body: text ? { text } : undefined }));
  }
  sleep() {
    return unwrap(this.api.POST("/sleep"));
  }
  say(text: string) {
    return unwrap(this.api.POST("/session/say", { body: { text } }));
  }
  turns() {
    return unwrap(this.api.GET("/session/turns")) as Promise<Turn[]>;
  }

  // --- writes --------------------------------------------------------------------
  pending() {
    return unwrap(this.api.GET("/proposals/pending"));
  }
  confirm() {
    return unwrap(this.api.POST("/proposals/pending/confirm"));
  }
  cancel() {
    return unwrap(this.api.POST("/proposals/pending/cancel"));
  }
  undo() {
    return unwrap(this.api.POST("/undo"));
  }
  audit(query: paths["/audit"]["get"]["parameters"]["query"] = {}) {
    return unwrap(this.api.GET("/audit", { params: { query } }));
  }

  // --- configuration ---------------------------------------------------------------
  abilities() {
    return unwrap(this.api.GET("/abilities"));
  }
  policy() {
    return unwrap(this.api.GET("/policy"));
  }
  updatePolicy(update: PolicyUpdate) {
    return unwrap(this.api.PUT("/policy", { body: update }));
  }
  settings() {
    return unwrap(this.api.GET("/settings"));
  }
  updateSettings(update: SettingsUpdate) {
    return unwrap(this.api.PUT("/settings", { body: update }));
  }
  devices() {
    return unwrap(this.api.GET("/devices"));
  }
  setupEchoCancel(uninstall = false) {
    return unwrap(this.api.POST("/setup/echo-cancel", { params: { query: { uninstall } } }));
  }
  models() {
    return unwrap(this.api.GET("/setup/models"));
  }
}

export class ApiError extends Error {
  constructor(
    public readonly status: number,
    public readonly detail: unknown,
  ) {
    super(typeof detail === "string" ? detail : `HTTP ${status}`);
  }
}

async function unwrap<T>(
  p: Promise<{ data?: T; error?: unknown; response: Response }>,
): Promise<T> {
  const { data, error, response } = await p;
  if (error !== undefined || !response.ok) {
    const detail = (error as { detail?: unknown } | undefined)?.detail ?? error;
    throw new ApiError(response.status, detail);
  }
  return data as T;
}

/** Plain HTTP + WebSocket transport. `baseUrl` like `http://localhost:5173/api`. */
export function httpTransport(baseUrl: string, fetchImpl: typeof fetch = fetch): Transport {
  const wsUrl = baseUrl.replace(/^http/, "ws").replace(/\/$/, "") + "/events";
  return {
    baseUrl,
    fetch: fetchImpl,
    events: {
      subscribe(onEvent, onClose) {
        let ws: WebSocket | null = new WebSocket(wsUrl);
        ws.onmessage = (m) => onEvent(JSON.parse(String(m.data)));
        ws.onclose = () => onClose?.();
        return () => {
          ws?.close();
          ws = null;
        };
      },
    },
  };
}
