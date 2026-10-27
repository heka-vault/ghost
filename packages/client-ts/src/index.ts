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
export type SourceInfo = Schemas["SourceInfo"];
export type OperationInfo = Schemas["OperationInfo"];
export type ReadToolInfo = Schemas["ReadToolInfo"];
export type McpServerInfo = Schemas["McpServerInfo"];
export type McpToolInfo = Schemas["McpToolInfo"];
export type McpWriteInfo = Schemas["McpWriteInfo"];
export type McpScopeInfo = Schemas["McpScopeInfo"];
export type PolicyView = Schemas["PolicyView"];
export type PolicyUpdate = Schemas["PolicyUpdate"];
export type PolicyResult = Schemas["PolicyResult"];
export type SettingsView = Schemas["SettingsView"];
export type SettingsUpdate = Schemas["SettingsUpdate"];
export type DevicesView = Schemas["DevicesView"];
export type ModelsView = Schemas["ModelsView"];
export type Ack = Schemas["Ack"];
/** An agent (ADR 0024): a conversation with a persona and a job, in a department. The wire keeps the word conversation. */
export type ConversationView = Schemas["ConversationView"];
export type ConversationDetail = Schemas["ConversationDetail"];
export type ConversationCreate = Schemas["ConversationCreate"];
export type ConversationUpdate = Schemas["ConversationUpdate"];
export type ConversationMove = Schemas["ConversationMove"];
export type TurnView = Schemas["TurnView"];
export type CompactionView = Schemas["CompactionView"];
export type VoiceInfo = Schemas["VoiceInfo"];
export type PhoneView = Schemas["PhoneView"];
export type PhoneDevice = Schemas["PhoneDevice"];
export type PhonePairing = Schemas["PhonePairing"];
export type SkillInfo = Schemas["SkillInfo"];
export type SkillBody = Schemas["SkillBody"];
export type SkillAdd = Schemas["SkillAdd"];
export type DepartmentView = Schemas["DepartmentView"];
export type DepartmentCreate = Schemas["DepartmentCreate"];
export type JournalEntry = Schemas["JournalEntry"];
export type RuleAdd = Schemas["RuleAdd"];
export type WaitingItemView = Schemas["WaitingItemView"];
export type WaitingView = Schemas["WaitingView"];
export type WatchInfo = Schemas["WatchInfo"];
export type HookInfo = Schemas["HookInfo"];
export type RoutesView = Schemas["RoutesView"];

/** Payloads on the /events WebSocket (first message is a plain Status snapshot). */
export type GhostEvent =
  | Schemas["StatusEvent"]
  | Schemas["TurnEvent"]
  | Schemas["PendingEvent"]
  | Schemas["OutcomeEvent"]
  | Schemas["LogEvent"]
  | Schemas["ConversationEvent"]
  | Schemas["WaitingEvent"];

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
  /** Open a session: with Ghost, or with the agent `conversation` names (a Call from its page). */
  wake(text?: string, conversation?: string) {
    const body = text || conversation ? { text: text || null, conversation: conversation || null } : undefined;
    return unwrap(this.api.POST("/wake", { body }));
  }
  sleep() {
    return unwrap(this.api.POST("/sleep"));
  }
  /** Type to the live agent; with no session running, open one (with `conversation`, or Ghost) with these words as its first turn. */
  say(text: string, conversation?: string) {
    return unwrap(this.api.POST("/session/say", { body: conversation ? { text, conversation } : { text } }));
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
  /** Confirm a change on the waiting list by id, with or without a session (ADR 0030). */
  confirmProposal(id: string) {
    return unwrap(this.api.POST("/proposals/{proposal_id}/confirm", { params: { path: { proposal_id: id } } }));
  }
  cancelProposal(id: string) {
    return unwrap(this.api.POST("/proposals/{proposal_id}/cancel", { params: { path: { proposal_id: id } } }));
  }

  // --- the waiting list (ADR 0029) ------------------------------------------------
  /** What needs the operator: items from watches and Ghost's own, with their state. */
  waiting() {
    return unwrap(this.api.GET("/waiting"));
  }
  /** The operator heard it: it stops alerting until it changes. */
  acknowledge(key: string) {
    return unwrap(this.api.POST("/waiting/ack", { body: { key } }));
  }
  snooze(key: string, minutes: number) {
    return unwrap(this.api.POST("/waiting/snooze", { body: { key, minutes } }));
  }
  audit(query: paths["/audit"]["get"]["parameters"]["query"] = {}) {
    return unwrap(this.api.GET("/audit", { params: { query } }));
  }

  // --- configuration ---------------------------------------------------------------
  /** Every source of tools the gate knows (ADR 0022): the agents and the departments, built in,
   *  and each MCP server, with its operations at their resolved levels and its allowlist. */
  sources() {
    return unwrap(this.api.GET("/sources"));
  }
  /** Every declared MCP server (ADR 0024): Ghost's and each department's own, connected or not,
   *  their tools, and the writes declared for the gate. Read-only; the folders are the place to change them. */
  mcp() {
    return unwrap(this.api.GET("/mcp"));
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

  // --- agents (ADR 0014, ADR 0024) ---------------------------------------------------------
  /** Every agent, Ghost first; retired ones have `retired_at` set. Pass `retired` to get only
   *  the retired or only the working ones, and `department` for one roster. */
  conversations(retired?: boolean, department?: string) {
    const query: { retired?: boolean; department?: string } = {};
    if (retired !== undefined) query.retired = retired;
    if (department) query.department = department;
    return unwrap(this.api.GET("/conversations", { params: { query } }));
  }
  conversation(slug: string) {
    return unwrap(this.api.GET("/conversations/{slug}", { params: { path: { slug } } }));
  }
  /** Hire an agent into a department. */
  createConversation(body: ConversationCreate) {
    return unwrap(this.api.POST("/conversations", { body }));
  }
  updateConversation(slug: string, body: ConversationUpdate) {
    return unwrap(this.api.PATCH("/conversations/{slug}", { params: { path: { slug } }, body }));
  }
  addNote(slug: string, text: string) {
    return unwrap(
      this.api.POST("/conversations/{slug}/notes", { params: { path: { slug } }, body: { text } }),
    );
  }
  deleteConversation(slug: string) {
    return unwrap(this.api.DELETE("/conversations/{slug}", { params: { path: { slug } } }));
  }
  resetConversation(slug: string) {
    return unwrap(this.api.POST("/conversations/{slug}/reset", { params: { path: { slug } } }));
  }
  switchConversation(slug: string) {
    return unwrap(this.api.POST("/conversations/{slug}/switch", { params: { path: { slug } } }));
  }
  /** Retire an agent (ADR 0024): off the roster and voice routing, everything kept.
   *  409 for a department's own agent or for the agent the session is with. */
  retireConversation(slug: string) {
    return unwrap(this.api.POST("/conversations/{slug}/retire", { params: { path: { slug } } }));
  }
  rehireConversation(slug: string) {
    return unwrap(this.api.POST("/conversations/{slug}/rehire", { params: { path: { slug } } }));
  }
  /** Move an agent's folder into another department (ADR 0024). 409 for a department's own
   *  agent, the agent the session is with, or the department it is already in. */
  moveConversation(slug: string, department: string) {
    return unwrap(
      this.api.POST("/conversations/{slug}/move", { params: { path: { slug } }, body: { department } }),
    );
  }
  compactConversation(slug: string) {
    return unwrap(this.api.POST("/conversations/{slug}/compact", { params: { path: { slug } } }));
  }
  conversationTurns(slug: string, limit = 200) {
    return unwrap(
      this.api.GET("/conversations/{slug}/turns", { params: { path: { slug }, query: { limit } } }),
    );
  }
  conversationCompactions(slug: string) {
    return unwrap(
      this.api.GET("/conversations/{slug}/compactions", { params: { path: { slug } } }),
    );
  }
  voices() {
    return unwrap(this.api.GET("/voices"));
  }
  sampleVoice(voice: string, speed = 1.0, text?: string) {
    return unwrap(
      this.api.POST("/voices/{voice}/sample", {
        params: { path: { voice } },
        body: { speed, text: text ?? null },
      }),
    );
  }

  // --- departments (ADR 0024) --------------------------------------------------------------
  /** Every department, Ghost first: its folder, rules, servers, skills, agents and problems. */
  departments() {
    return unwrap(this.api.GET("/departments"));
  }
  department(slug: string) {
    return unwrap(this.api.GET("/departments/{slug}", { params: { path: { slug } } }));
  }
  /** A new department folder with its AGENTS.md, mcp.json and department.json. 409 when the name is taken. */
  createDepartment(body: DepartmentCreate) {
    return unwrap(this.api.POST("/departments", { body }));
  }
  /** Append a rule to the department's AGENTS.md, verbatim, through the gate. 409 when its rules live in a project folder. */
  addRule(slug: string, text: string) {
    return unwrap(this.api.POST("/departments/{slug}/rules", { params: { path: { slug } }, body: { text } }));
  }
  /** What the department's agents did, newest first, from the audit. */
  departmentJournal(slug: string, limit = 30) {
    return unwrap(this.api.GET("/departments/{slug}/journal", { params: { path: { slug }, query: { limit } } }));
  }

  // --- phone (ADR 0016) ----------------------------------------------------------------
  /** The phone listener's address (null when GHOST_PHONE_LISTEN is unset) and the paired phone. */
  phone() {
    return unwrap(this.api.GET("/phone"));
  }
  /** A new six-digit pairing code; replaces the last one. 409 when the listener is off. */
  pairPhone() {
    return unwrap(this.api.POST("/phone/pairing"));
  }
  /** Forget the paired phone. Its signatures stop working and a running phone session ends. */
  forgetPhone() {
    return unwrap(this.api.DELETE("/phone"));
  }

  // --- skills (ADR 0023, ADR 0024) -----------------------------------------------------------
  /** Every SKILL.md folder Ghost reads for itself: its own and the operator's, with the servers
   *  each needs, which of those aren't connected, and any problem that keeps it from being offered. */
  skills() {
    return unwrap(this.api.GET("/skills"));
  }
  /** The SKILL.md text without its front matter, and the files beside it. */
  skill(name: string) {
    return unwrap(this.api.GET("/skills/{name}", { params: { path: { name } } }));
  }
  /** Copy a folder on this machine into Ghost's skills. 409 with a message when it is not
   *  a skill or already installed; `replace` overwrites one already installed. */
  addSkill({ path, name, replace }: { path: string; name?: string | null; replace?: boolean }) {
    return unwrap(this.api.POST("/skills", { body: { path, name: name ?? null, replace: replace ?? false } }));
  }
  /** Delete one of Ghost's own skills. 409 for one read in place from elsewhere. */
  removeSkill(name: string) {
    return unwrap(this.api.DELETE("/skills/{name}", { params: { path: { name } } }));
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
