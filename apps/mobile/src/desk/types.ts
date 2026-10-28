/**
 * What the phone needs from the desk (the workstation running ghostd), and nothing more.
 *
 * The phone never uses the control API (ADR 0013). It joins the voice pipeline as a
 * WebRTC transport, so the gate, the policy and Jev apply unchanged, and everything below
 * arrives over that connection. Screens read only this interface: `MockDesk` stands in
 * until the transport exists, then a WebRTC desk replaces it without touching a screen.
 *
 * There is no confirm() here on purpose. The phone confirms by voice: the operator says
 * yes, the pipeline hears it, Jev classifies it, the gate writes. There is no undo either:
 * to change something back the operator asks Ghost, which makes the opposite change through
 * the gate like any other.
 */
import type { AuditEntry, ConversationView, OutcomeView, PendingProposal, Status, Turn, WaitingItemView } from "@ghost/client-ts";

/** How the phone reaches the desk right now. */
export type Link = "connecting" | "online" | "reconnecting" | "offline";

/** Which of the desk's addresses answered: the home Wi-Fi one, or the tunnel used away (ADR 0020). */
export type Via = "Wi-Fi" | "tunnel";

/** Where the call's audio goes on the phone. */
export type Route = "speaker" | "earpiece" | "bluetooth" | "headset";

export interface AudioState {
  route: Route;
  /** What the phone can route to right now; Bluetooth and a headset appear when connected. */
  available: Route[];
}

/** What the call looked like, read from the phone's own WebRTC stats when it ended (ADR 0018). */
export interface CallStats {
  via: Via;
  endedAt: string;
  seconds: number;
  /** Packets lost on the way to the phone, as a percentage; null if unknown. */
  lossPct: number | null;
  jitterMs: number | null;
  rttMs: number | null;
}

export interface DeskState {
  link: Link;
  /** Which address is in use; null until one answers. */
  via: Via | null;
  /** That address itself (host:port), shown in Settings; null until one answers. */
  address: string | null;
  /** The last call this phone made, if any. */
  lastCall: CallStats | null;
  /** The call's audio route; speaker by default, a car or headset when one is connected. */
  audio: AudioState;
  /** The threads a call can start in, Ghost first, settled ones left out (ADR 0014). */
  conversations: ConversationView[];
  /** The daemon's status as it last reported it; null before the first one arrives. */
  status: Status | null;
  /** The confirm-level change waiting for a spoken yes, if any. */
  pending: PendingProposal | null;
  /** This session's turns, oldest first. Cleared when the session ends. */
  turns: Turn[];
  /** The newest audit entries, newest first; null until the first page arrives. */
  audit: AuditEntry[] | null;
  /** How many entries the desk has in total, for “Show older”. */
  auditTotal: number;
  /** Outcomes by proposal id, as the gate reports them. */
  outcomes: Record<string, OutcomeView>;
  /** What needs the operator (ADR 0029), oldest first; null until the first read. Read
   *  while the app is open: there is no push (2026-10-27). */
  waiting: WaitingItemView[] | null;
}

export interface Desk {
  getState(): DeskState;
  subscribe(listener: () => void): () => void;
  /** Open the phone's mic into a session on the desk, in Ghost or in the thread named. */
  start(conversation?: string): Promise<void>;
  /** Close the session and the phone's mic. */
  end(): Promise<void>;
  /** Ask the desk once, now; the link also keeps retrying on its own. */
  retry(): Promise<void>;
  /** Ask for more of the audit log. */
  loadAudit(limit: number): Promise<void>;
  /** The operator heard it: it stops reminding until it changes. Not a write anywhere, so a
   *  tap is fine; a change waiting for a yes is still answered by voice. */
  acknowledge(key: string): Promise<void>;
  /** Not now: it comes back after `minutes`. */
  snooze(key: string, minutes: number): Promise<void>;
  /** Stop or resume sending the phone's mic; the session stays open. */
  setMuted(muted: boolean): void;
  /** Send the call's audio somewhere else (speaker, Bluetooth, headset, earpiece). */
  setRoute(route: Route): void;
}
