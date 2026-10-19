/**
 * The real desk: ghostd's phone listener on the LAN (ADR 0016, docs/phone-protocol.md).
 *
 * Outside a session the phone asks `/hello` every few seconds for the link and the status,
 * and `/audit` for the Activity tab. Start opens the mic and joins the pipeline over WebRTC.
 * From then on the desk's own session events arrive on the data channel, as the same JSON the
 * desktop app reads. Every request is signed, and so is every reply. An answer whose signature
 * doesn't check is never applied, which is what keeps the audio between this phone and its desk.
 */
import type { AuditPage, GhostEvent, Status } from "@ghost/client-ts";
import { PermissionsAndroid, Platform } from "react-native";
import InCallManager from "react-native-incall-manager";
import { mediaDevices, MediaStream, RTCPeerConnection, RTCSessionDescription } from "react-native-webrtc";

import { signRequest, verifyResponse } from "../phone/signing";
import type { Desk, DeskState, Link } from "./types";

const POLL_MS = 4000;
const POLL_IN_SESSION_MS = 15000;
const REQUEST_MS = 8000;
const GATHER_MS = 3000;
const JOIN_MS = 15000;
/** Missed /hello answers before the link shows offline rather than reconnecting. */
const OFFLINE_AFTER = 3;

type DataChannel = ReturnType<RTCPeerConnection["createDataChannel"]>;

export interface LiveDeskConfig {
  address: string;
  deviceId: string;
  secret: Uint8Array;
}

class DeskError extends Error {}

const MESSAGES: Record<string, string> = {
  busy: "Ghost is busy at your desk. End that session there, then start here.",
  closing: "Your last session is still closing on the desk. Try again in a few seconds.",
  not_ready: "Ghost is still loading its voice models. Try again in a moment.",
  unauthorized: "Your desk doesn’t recognise this phone any more. Pair it again in Settings.",
};

export class LiveDesk implements Desk {
  private state: DeskState = { link: "connecting", status: null, pending: null, turns: [], audit: null, auditTotal: 0, outcomes: {} };
  private listeners = new Set<() => void>();
  private failures = 0;
  private poll: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;
  private auditLimit = 30;

  private pc: RTCPeerConnection | null = null;
  private dc: DataChannel | null = null;
  private mic: MediaStream | null = null;
  private ping: ReturnType<typeof setInterval> | null = null;
  private joined: (() => void) | null = null;

  constructor(private cfg: LiveDeskConfig) {
    void this.tick();
  }

  getState = () => this.state;

  subscribe = (l: () => void) => {
    this.listeners.add(l);
    return () => {
      this.listeners.delete(l);
    };
  };

  /** Stop polling and drop any session; the desk is being replaced (unpaired or re-paired). */
  dispose() {
    this.disposed = true;
    if (this.poll) clearTimeout(this.poll);
    this.teardown();
  }

  private set(patch: Partial<DeskState>) {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  // ------------------------------------------------------------------ signed HTTP

  private async request<T>(method: "GET" | "POST" | "PATCH", path: string, body?: unknown): Promise<T> {
    const text = body === undefined ? "" : JSON.stringify(body);
    const { headers, nonce } = signRequest(this.cfg.deviceId, this.cfg.secret, method, path, text);
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), REQUEST_MS);
    let res: Response;
    try {
      res = await fetch(`http://${this.cfg.address}${path}`, {
        method,
        headers: text ? { ...headers, "Content-Type": "application/json" } : headers,
        body: text || undefined,
        signal: abort.signal,
      });
    } catch (e) {
      console.warn(`desk: ${method} ${path} failed: ${e instanceof Error ? `${e.name}: ${e.message}` : String(e)}`);
      throw new DeskError("Your desk isn’t reachable right now.");
    } finally {
      clearTimeout(timer);
    }
    const raw = await res.text();
    if (!verifyResponse(this.cfg.secret, res.status, nonce, res.headers.get("X-Ghost-Time"), res.headers.get("X-Ghost-Signature"), raw)) {
      // An unsigned 401 is still worth reading: the desk forgot this phone.
      throw new DeskError(res.status === 401 ? MESSAGES.unauthorized : "Your desk’s reply didn’t check out, so the phone ignored it.");
    }
    const data = raw ? JSON.parse(raw) : null;
    if (!res.ok) {
      let code = (data as { error?: string } | null)?.error ?? "";
      if (code === "busy" && this.state.status?.session_origin !== "desk") code = "closing";
      throw new DeskError(MESSAGES[code] ?? `Your desk said no (${res.status}).`);
    }
    return data as T;
  }

  // ------------------------------------------------------------------ the link

  private async tick() {
    if (this.disposed) return;
    try {
      const hello = await this.request<{ workstation: string; status: Status }>("GET", "/hello");
      this.failures = 0;
      const first = this.state.audit === null;
      const wasSession = !!this.state.status?.session_id;
      this.onStatus(hello.status);
      this.set({ link: "online" });
      if (first || (wasSession && !hello.status.session_id)) void this.loadAudit(this.auditLimit).catch(() => {});
    } catch {
      this.failures += 1;
      const link: Link = this.failures >= OFFLINE_AFTER || this.state.link === "connecting" ? "offline" : "reconnecting";
      // In a session the data channel is the better witness; leave the link alone while it is open.
      if (this.dc?.readyState !== "open") this.set({ link });
    }
    if (this.disposed) return;
    this.poll = setTimeout(() => void this.tick(), this.pc ? POLL_IN_SESSION_MS : POLL_MS);
  }

  async retry() {
    if (this.poll) clearTimeout(this.poll);
    await this.tick();
    if (this.state.link !== "online") throw new DeskError("Still can’t reach it.");
  }

  async loadAudit(limit: number) {
    this.auditLimit = Math.max(this.auditLimit, limit);
    const page = await this.request<AuditPage>("GET", `/audit?limit=${limit}&offset=0`);
    this.set({ audit: page.items, auditTotal: page.total });
  }

  // ------------------------------------------------------------------ desk events

  private onStatus(status: Status) {
    const wasOurs = !!this.pc && !!this.state.status?.session_id;
    this.set({ status, pending: status.pending ?? null });
    // The desk ended the session (Ghost said goodbye, or the operator ended it there).
    if (wasOurs && !status.session_id) this.teardown();
    if (status.session_id && status.session_origin === "phone" && this.joined) {
      this.joined();
      this.joined = null;
    }
  }

  private onEvent(e: GhostEvent) {
    switch (e.type) {
      case "status":
        return this.onStatus(e.status);
      case "turn":
        // Turns from before the channel opened come ahead of the first status, so trust the call.
        if (this.pc) this.set({ turns: [...this.state.turns, e.turn] });
        return;
      case "pending":
        return this.set({ pending: e.pending ?? null });
      case "outcome":
        this.set({ outcomes: { ...this.state.outcomes, [e.outcome.proposal_id]: e.outcome } });
        void this.loadAudit(this.auditLimit).catch(() => {});
        return;
      case "conversation":
        if (this.state.status) this.set({ status: { ...this.state.status, conversation: e.active ?? null } });
        return;
    }
  }

  private onMessage(raw: unknown) {
    if (typeof raw !== "string" || !raw.startsWith("{")) return;
    let msg: { label?: string; type?: string; data?: unknown };
    try {
      msg = JSON.parse(raw);
    } catch {
      return;
    }
    if (msg.label !== "rtvi-ai" || msg.type !== "server-message" || !msg.data) return;
    this.onEvent(msg.data as GhostEvent);
  }

  // ------------------------------------------------------------------ the session

  async start() {
    if (this.state.link !== "online") throw new DeskError("Your desk isn’t reachable right now.");
    if (this.state.status?.session_origin === "desk") throw new DeskError(MESSAGES.busy);
    if (this.pc) return;

    if (Platform.OS === "android") {
      const ok = await PermissionsAndroid.request(PermissionsAndroid.PERMISSIONS.RECORD_AUDIO);
      if (ok !== PermissionsAndroid.RESULTS.GRANTED) throw new DeskError("Ghost needs the microphone to hear you. Allow it for Ghost in Android settings.");
    }

    try {
      // The Android module turns on the hardware echo canceller and noise suppressor by
      // itself; on speakerphone some of Ghost's voice still leaks, and the desk drops it
      // (ghost_voice.echo).
      this.mic = (await mediaDevices.getUserMedia({ audio: true, video: false })) as MediaStream;
      const pc = new RTCPeerConnection({ iceServers: [] });
      this.pc = pc;
      const dc = pc.createDataChannel("chat", { ordered: true });
      this.dc = dc;
      dc.addEventListener("open", () => {
        dc.send(JSON.stringify({ label: "rtvi-ai", type: "client-ready", id: rid(), data: { version: "2.1.0", about: { library: "ghost-mobile" } } }));
        this.ping = setInterval(() => dc.readyState === "open" && dc.send("ping"), 1000);
      });
      dc.addEventListener("message", (e: { data: unknown }) => this.onMessage(e.data));
      pc.addEventListener("connectionstatechange", () => {
        const s = pc.connectionState;
        if (s === "disconnected") this.set({ link: "reconnecting" });
        else if (s === "connected") this.set({ link: "online" });
        else if (s === "failed" || s === "closed") this.endedUnderUs();
      });
      this.mic.getTracks().forEach((t) => pc.addTrack(t, this.mic!));

      await pc.setLocalDescription(await pc.createOffer({}));
      await gathered(pc);
      const local = pc.localDescription!;
      this.set({ turns: [], outcomes: {} });
      const answer = await this.request<{ sdp: string; type: "answer"; pc_id: string }>("POST", "/offer", { sdp: local.sdp, type: local.type });
      await pc.setRemoteDescription(new RTCSessionDescription({ sdp: answer.sdp, type: answer.type }));

      InCallManager.start({ media: "audio" });
      InCallManager.setForceSpeakerphoneOn(true);

      await new Promise<void>((resolve, reject) => {
        const t = setTimeout(() => reject(new DeskError("Your desk answered but the session didn’t open. Try again.")), JOIN_MS);
        this.joined = () => {
          clearTimeout(t);
          resolve();
        };
        if (this.state.status?.session_origin === "phone" && this.state.status.session_id) this.joined();
      });
    } catch (e) {
      this.joined = null;
      const offered = !!this.pc?.remoteDescription;
      this.teardown();
      if (offered) void this.request("POST", "/end", {}).catch(() => {});
      throw e instanceof DeskError ? e : new DeskError(`Couldn’t open the call to your desk: ${e instanceof Error ? e.message : String(e)}`);
    }
  }

  async end() {
    if (!this.pc) return;
    // Say goodbye on the call first: it is already open and reaches the desk at once. The
    // POST is the record that the phone asked; the summary says so if it didn't arrive.
    try {
      if (this.dc?.readyState === "open") this.dc.send(JSON.stringify({ label: "rtvi-ai", type: "disconnect-bot", id: rid(), data: {} }));
    } catch {
      // the channel closed under us; the POST still tells the desk
    }
    try {
      await this.request("POST", "/end", {});
    } finally {
      this.teardown();
    }
  }

  setMuted(muted: boolean) {
    this.mic?.getAudioTracks().forEach((t) => {
      t.enabled = !muted;
    });
  }

  /** The call dropped and didn't come back. The desk ends its side when the peer goes. */
  private endedUnderUs() {
    if (!this.pc) return;
    this.teardown();
    // Tell the desk too, so it doesn't wait out its ping timeout before Start works again.
    void this.request("POST", "/end", {}).catch(() => {});
    void this.tick();
  }

  private teardown() {
    if (this.ping) clearInterval(this.ping);
    this.ping = null;
    const { pc, dc, mic } = this;
    this.pc = null;
    this.dc = null;
    this.mic = null;
    try {
      dc?.close();
      pc?.close();
    } catch {
      // already closed
    }
    mic?.getTracks().forEach((t) => t.stop());
    if (pc) {
      try {
        InCallManager.stop();
      } catch {
        // never started
      }
    }
    if (this.state.status?.session_origin === "phone") {
      this.set({ status: { ...this.state.status, session_id: null, session_origin: null, phase: "passive", pending: null }, pending: null, turns: [] });
    }
  }
}

const rid = () => Math.random().toString(36).slice(2, 10);

/** Wait until ICE gathering finishes so the offer carries every host candidate; no trickle. */
function gathered(pc: RTCPeerConnection): Promise<void> {
  if (pc.iceGatheringState === "complete") return Promise.resolve();
  return new Promise((resolve) => {
    const done = () => {
      clearTimeout(t);
      resolve();
    };
    const t = setTimeout(done, GATHER_MS);
    pc.addEventListener("icegatheringstatechange", () => {
      if (pc.iceGatheringState === "complete") done();
    });
  });
}
