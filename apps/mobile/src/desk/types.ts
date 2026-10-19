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
import type { AuditEntry, OutcomeView, PendingProposal, Status, Turn } from "@ghost/client-ts";

/** How the phone reaches the desk right now. */
export type Link = "connecting" | "online" | "reconnecting" | "offline";

export interface DeskState {
  link: Link;
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
}

export interface Desk {
  getState(): DeskState;
  subscribe(listener: () => void): () => void;
  /** Open the phone's mic into a session on the desk. */
  start(): Promise<void>;
  /** Close the session and the phone's mic. */
  end(): Promise<void>;
  /** Ask the desk once, now; the link also keeps retrying on its own. */
  retry(): Promise<void>;
  /** Ask for more of the audit log. */
  loadAudit(limit: number): Promise<void>;
  /** Stop or resume sending the phone's mic; the session stays open. */
  setMuted(muted: boolean): void;
}
