/**
 * The real desk: ghostd's phone listener (ADR 0016, docs/phone-protocol.md), on the home
 * Wi-Fi or through the operator's tunnel from anywhere (ADR 0020).
 *
 * The desk lists every address it serves; the phone asks all of them once and keeps the
 * first in the list that answers (home before tunnel), until that one stops answering.
 * Outside a session the phone asks `/hello` every few seconds for the link and the status,
 * and `/audit` for the Activity tab. Start opens the mic and joins the pipeline over WebRTC.
 * From then on the desk's own session events arrive on the data channel, as the same JSON the
 * desktop app reads. Every request is signed, and so is every reply. An answer whose signature
 * doesn't check is never applied, which is what keeps the audio between this phone and its desk.
 */
import type { AuditPage, ConversationView, GhostEvent, Status, WaitingView } from "@ghost/client-ts";
import { lineName } from "@ghost/ui";
import { DeviceEventEmitter, type EmitterSubscription, PermissionsAndroid, Platform } from "react-native";
import InCallManager from "react-native-incall-manager";
import { mediaDevices, MediaStream, RTCPeerConnection, RTCSessionDescription } from "react-native-webrtc";

import { onCallEnd, onCallTick, startCall, stopCall, updateCall } from "../../modules/ghost-call";
import { rememberAddresses } from "../phone/pairing";
import { signRequest, verifyResponse } from "../phone/signing";
import type { AudioState, CallStats, Desk, DeskState, Link, Route, Via } from "./types";

const POLL_MS = 4000;
const WAITING_EVERY_TICKS = 8; // ~30 s: the desk counts the phone as showing the list while it reads it
const POLL_IN_SESSION_MS = 15000;
const REQUEST_MS = 8000;
const GATHER_MS = 3000;
const JOIN_MS = 15000;
/** Once everything is up, a moment for the audio route to settle before the desk greets. */
const READY_SETTLE_MS = 250;
/**
 * How long the phone waits for the audio route to settle before saying client-ready. A
 * Bluetooth headset takes a second or two to bring its call audio up after the call starts;
 * greeting before that put the first words on the speaker and lost the last ones in the
 * switch ("System persona he-").
 */
const ROUTE_SETTLE_MS = 2200;
/** The route counts as settled once no device change has arrived for this long. */
const ROUTE_QUIET_MS = 600;
/** Missed /hello answers before the link shows offline rather than reconnecting. */
const OFFLINE_AFTER = 3;
/** How long the phone gives every address to answer when it is choosing one. */
const PICK_MS = 3000;
/** The home address gets this much of a head start before a tunnel answer is taken. */
const PICK_HOME_LEAD_MS = 400;
/** After a goodbye, how long the phone waits for it to finish playing before closing the call. */
const HANG_UP_MS = 3500;
/**
 * The desk says "listening" when it has finished sending the goodbye; the phone is still
 * playing the last of it (jitter buffer and the trip over). Closing on the word cut "Bye for
 * now" to "bye for n"; this is the tail the phone lets play first.
 */
const HANG_UP_TAIL_MS = 600;
/** After the phone hung up, a stale "session running" from the desk is ignored this long. */
const JUST_ENDED_MS = 8000;

/** InCallManager's names for the routes. */
const ROUTE_NAMES: Record<Route, string> = { speaker: "SPEAKER_PHONE", earpiece: "EARPIECE", bluetooth: "BLUETOOTH", headset: "WIRED_HEADSET" };
const ROUTE_OF: Record<string, Route> = { SPEAKER_PHONE: "speaker", EARPIECE: "earpiece", BLUETOOTH: "bluetooth", WIRED_HEADSET: "headset" };
const DEFAULT_AUDIO: AudioState = { route: "speaker", available: ["speaker"] };

type AudioDeviceEvent = { availableAudioDeviceList?: string; selectedAudioDevice?: string };

type DataChannel = ReturnType<RTCPeerConnection["createDataChannel"]>;

export interface LiveDeskConfig {
  /** The desk's addresses, home first (ADR 0020). */
  addresses: string[];
  deviceId: string;
  secret: Uint8Array;
}

class DeskError extends Error {}

const MESSAGES: Record<string, string> = {
  busy: "Ghost is busy at your desk. End that session there, then start here.",
  closing: "Your last session is still closing on the desk. Try again in a few seconds.",
  not_ready: "Ghost is still loading its voice models. Try again in a moment.",
  unauthorized: "Your desk doesn’t recognise this phone any more. Pair it again in Settings.",
  no_such_line: "That agent isn’t there any more. Pull the list again and try another.",
};

export class LiveDesk implements Desk {
  private state: DeskState = { link: "connecting", via: null, lastCall: null, audio: DEFAULT_AUDIO, conversations: [], status: null, pending: null, turns: [], audit: null, auditTotal: 0, outcomes: {}, waiting: null };
  /** The address in use; null until one answers, and again once it stops answering. */
  private address: string | null = null;
  private listeners = new Set<() => void>();
  private failures = 0;
  private poll: ReturnType<typeof setTimeout> | null = null;
  private disposed = false;
  private auditLimit = 30;
  /** The waiting count the desk last reported, and ticks since the list was read. */
  private waitingSeen: number | null = null;
  private ticksSinceWaiting = 0;

  private pc: RTCPeerConnection | null = null;
  private dc: DataChannel | null = null;
  private mic: MediaStream | null = null;
  private ping: ReturnType<typeof setInterval> | null = null;
  private joined: (() => void) | null = null;
  private callStartedAt = 0;
  private endedAt = 0;
  private goodbyeHeard = false;
  private hangUpTimer: ReturnType<typeof setTimeout> | null = null;
  private tailTimer: ReturnType<typeof setTimeout> | null = null;
  private routeSub: EmitterSubscription | null = null;
  private routeSettled: (() => void) | null = null;
  private routeQuiet: ReturnType<typeof setTimeout> | null = null;
  private readonly endSub: () => void;
  private readonly tickSub: () => void;

  constructor(private cfg: LiveDeskConfig) {
    // End on the session notification (screen off, phone in a pocket) ends the call like the button.
    this.endSub = onCallEnd(() => void this.end().catch(() => {}));
    // The keepalive the desk expects every second. JS timers stop in the background (the
    // desk hung up 15 s after the app left the screen); the service's tick does not.
    this.tickSub = onCallTick(() => this.keepalive());
    void this.tick();
  }

  private keepalive() {
    const dc = this.dc;
    if (dc?.readyState === "open") {
      try {
        dc.send("ping");
      } catch {
        // closing under us; the state change handles it
      }
    }
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
    this.endSub();
    this.tickSub();
    this.teardown();
  }

  private set(patch: Partial<DeskState>) {
    this.state = { ...this.state, ...patch };
    this.listeners.forEach((l) => l());
  }

  // ------------------------------------------------------------------ signed HTTP

  private async request<T>(method: "GET" | "POST" | "PATCH", path: string, body?: unknown): Promise<T> {
    return this.requestAt(this.address ?? this.cfg.addresses[0], method, path, body, REQUEST_MS);
  }

  private async requestAt<T>(address: string, method: "GET" | "POST" | "PATCH", path: string, body: unknown, ms: number): Promise<T> {
    const text = body === undefined ? "" : JSON.stringify(body);
    const { headers, nonce } = signRequest(this.cfg.deviceId, this.cfg.secret, method, path, text);
    const abort = new AbortController();
    const timer = setTimeout(() => abort.abort(), ms);
    let res: Response;
    try {
      res = await fetch(`http://${address}${path}`, {
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

  /**
   * Ask every address at once and keep the first that answers, with the home address
   * given a short head start so Wi-Fi wins at home even when the tunnel also answers.
   * Waiting for every address to settle made the first Home take the full timeout
   * whenever one path was dark. Null when none answers.
   */
  private async pick(): Promise<Hello | null> {
    const tries = this.cfg.addresses.map((a, i) =>
      this.requestAt<Hello>(a, "GET", "/hello", undefined, PICK_MS)
        .then(async (hello) => {
          if (i > 0) await new Promise((r) => setTimeout(r, PICK_HOME_LEAD_MS));
          return { a, hello };
        }),
    );
    let first: { a: string; hello: Hello } | null = null;
    try {
      first = await Promise.any(tries);
    } catch {
      return null;
    }
    this.address = first.a;
    this.set({ via: this.viaOf(first.a) });
    return first.hello;
  }

  private viaOf(address: string): Via {
    return address === this.cfg.addresses[0] ? "Wi-Fi" : "tunnel";
  }

  private async tick() {
    if (this.disposed) return;
    try {
      const hello = this.address ? await this.request<Hello>("GET", "/hello") : await this.pick();
      if (!hello) throw new DeskError("Your desk isn’t reachable right now.");
      // The desk may have gained or lost an address since pairing (a tunnel set up later).
      if (hello.addresses?.length) void rememberAddresses(hello.addresses).catch(() => {});
      this.failures = 0;
      const first = this.state.audit === null;
      const wasSession = !!this.state.status?.session_id;
      this.onStatus(hello.status);
      this.set({ link: "online" });
      if (first || (wasSession && !hello.status.session_id)) {
        void this.loadAudit(this.auditLimit).catch(() => {});
        void this.loadLines().catch(() => {});
      }
      // The waiting list (ADR 0029): when the count moved, and now and then, so the desk
      // knows this phone has it on screen.
      this.ticksSinceWaiting += 1;
      if (hello.status.waiting !== this.waitingSeen || this.ticksSinceWaiting >= WAITING_EVERY_TICKS) {
        this.waitingSeen = hello.status.waiting;
        void this.loadWaiting().catch(() => {});
      }
    } catch {
      this.failures += 1;
      const link: Link = this.failures >= OFFLINE_AFTER || this.state.link === "connecting" ? "offline" : "reconnecting";
      // In a session the data channel is the better witness; leave the link alone while it is open.
      if (this.dc?.readyState !== "open") {
        this.set({ link });
        // Off the call, a path that stopped answering is dropped at once so the next tick
        // chooses again (the phone left the house, or came home). At home a blip costs one
        // three-second pick that lands on the same address.
        if (!this.pc) {
          this.address = null;
          this.set({ via: null });
        }
      }
    }
    if (this.disposed) return;
    this.poll = setTimeout(() => void this.tick(), this.pc ? POLL_IN_SESSION_MS : POLL_MS);
  }

  async retry() {
    if (this.poll) clearTimeout(this.poll);
    await this.tick();
    if (this.state.link !== "online") throw new DeskError("Still can’t reach it.");
  }

  /** The threads a call can start in; Ghost first. Refreshed with the audit, and when one changes. */
  private async loadLines() {
    const lines = await this.request<ConversationView[]>("GET", "/conversations");
    this.set({ conversations: lines.filter((c) => !c.retired_at) });
  }

  async loadAudit(limit: number) {
    this.auditLimit = Math.max(this.auditLimit, limit);
    const page = await this.request<AuditPage>("GET", `/audit?limit=${limit}&offset=0`);
    this.set({ audit: page.items, auditTotal: page.total });
  }

  private async loadWaiting() {
    this.ticksSinceWaiting = 0;
    const view = await this.request<WaitingView>("GET", "/waiting");
    this.set({ waiting: view.items });
  }

  async acknowledge(key: string) {
    await this.request("POST", "/waiting/ack", { key });
    await this.loadWaiting();
  }

  async snooze(key: string, minutes: number) {
    await this.request("POST", "/waiting/snooze", { key, minutes });
    await this.loadWaiting();
  }

  // ------------------------------------------------------------------ desk events

  private onStatus(status: Status) {
    // The desk takes a moment to wind a session down after the phone hangs up; a poll in
    // that window still says "in a session". Our own end is the truth until then.
    if (!this.pc && status.session_origin === "phone" && status.session_id && Date.now() - this.endedAt < JUST_ENDED_MS) {
      status = { ...status, session_id: null, session_origin: null, phase: "passive", pending: null, ending: false };
    }
    const wasOurs = !!this.pc && !!this.state.status?.session_id;
    const wasHold = this.state.status?.phase === "hold";
    this.set({ status, pending: status.pending ?? null });
    // The notification says when Ghost is waiting to be addressed by name (ADR 0021).
    if (this.pc && (status.phase === "hold") !== wasHold) updateCall(this.callBody());
    // The desk ended the session (Ghost said goodbye, or the operator ended it there).
    if (wasOurs && !status.session_id) return this.teardown();
    if (this.pc && status.ending) this.hangingUp(status);
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
      case "waiting":
        void this.loadWaiting().catch(() => {});
        return;
      case "conversation":
        if (this.state.status) this.set({ status: { ...this.state.status, conversation: e.active ?? null } });
        if (this.pc) updateCall(this.callBody());
        if (e.changed) void this.loadLines().catch(() => {});  // a thread was created, edited or settled
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

  async start(conversation?: string) {
    if (this.state.link !== "online") throw new DeskError("Your desk isn’t reachable right now.");
    if (this.state.status?.session_origin === "desk") throw new DeskError(MESSAGES.busy);
    if (this.pc) return;

    if (Platform.OS === "android") {
      const ok = await PermissionsAndroid.request(PermissionsAndroid.PERMISSIONS.RECORD_AUDIO);
      if (ok !== PermissionsAndroid.RESULTS.GRANTED) throw new DeskError("Ghost needs the microphone to hear you. Allow it for Ghost in Android settings.");
      // Best effort, asked once: the session notification (Android 13+) and seeing a car's or
      // headset's Bluetooth (Android 12+). Either refused still leaves a working call.
      try {
        await PermissionsAndroid.requestMultiple([PermissionsAndroid.PERMISSIONS.POST_NOTIFICATIONS, PermissionsAndroid.PERMISSIONS.BLUETOOTH_CONNECT]);
      } catch {
        // an older Android without these permissions
      }
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
      // client-ready is the desk's cue to greet, so it waits for everything the greeting
      // needs: the channel open, Ghost's audio track here, and the speaker route up.
      let channelOpen = false;
      let trackHere = false;
      let routeUp = false;
      let readySent = false;
      const maybeReady = () => {
        if (readySent || !channelOpen || !trackHere || !routeUp) return;
        readySent = true;
        setTimeout(() => {
          if (dc.readyState === "open") dc.send(JSON.stringify({ label: "rtvi-ai", type: "client-ready", id: rid(), data: { version: "2.1.0", about: { library: "ghost-mobile" } } }));
        }, READY_SETTLE_MS);
      };
      dc.addEventListener("open", () => {
        channelOpen = true;
        // In the foreground this and the service's tick both ping; a ping is a few bytes and
        // the desk only needs one a second. Alone, this one stops in the background.
        this.ping = setInterval(() => this.keepalive(), 1000);
        maybeReady();
      });
      pc.addEventListener("track", () => {
        trackHere = true;
        maybeReady();
      });
      dc.addEventListener("message", (e: { data: unknown }) => this.onMessage(e.data));
      pc.addEventListener("connectionstatechange", () => {
        const s = pc.connectionState;
        // While Ghost hangs up, the desk closing the call is the end, not a dropout.
        if (s === "disconnected" && this.state.status?.ending) this.endedUnderUs();
        else if (s === "disconnected") this.set({ link: "reconnecting" });
        else if (s === "connected") this.set({ link: "online" });
        else if (s === "failed" || s === "closed") this.endedUnderUs();
      });
      this.mic.getTracks().forEach((t) => pc.addTrack(t, this.mic!));

      await pc.setLocalDescription(await pc.createOffer({}));
      await gathered(pc);
      this.callStartedAt = Date.now();
      const local = pc.localDescription!;
      this.set({ turns: [], outcomes: {} });
      const line = conversation && conversation !== "ghost" ? conversation : undefined;
      const answer = await this.request<{ sdp: string; type: "answer"; pc_id: string }>("POST", "/offer", { sdp: local.sdp, type: local.type, ...(line ? { conversation: line } : {}) });
      await pc.setRemoteDescription(new RTCSessionDescription({ sdp: answer.sdp, type: answer.type }));

      // "video" to InCallManager means: the speaker by default, a Bluetooth or wired headset
      // whenever one is connected, and no proximity sensor. Nothing is forced, so a car's
      // hands-free takes the call and the button on screen can move it.
      this.routeSub = DeviceEventEmitter.addListener("onAudioDeviceChanged", (e: AudioDeviceEvent) => this.onRoute(e));
      const settled = new Promise<void>((resolve) => {
        this.routeSettled = resolve;
      });
      InCallManager.start({ media: "video" });
      // Greet only once the audio is where it will stay: a headset's call audio up, or the
      // speaker when there is none. Capped, so a quiet headset never delays the greeting long.
      await Promise.race([settled, new Promise<void>((r) => setTimeout(r, ROUTE_SETTLE_MS))]);
      this.routeSettled = null;
      if (this.routeQuiet) clearTimeout(this.routeQuiet);
      this.routeQuiet = null;
      routeUp = true;
      maybeReady();
      // Hold the mic open with the screen off, and put an End action in the notification shade.
      startCall("Ghost", this.callBody());

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

  async end(tone = false) {
    if (!this.pc) return;
    this.endedAt = Date.now();
    // Say goodbye on the call and close it at once: the desk ends the session when the
    // phone goes, so End never waits on the network. The POST after is the record that the
    // phone asked; if it fails the summary says the desk may not have heard.
    try {
      if (this.dc?.readyState === "open") this.dc.send(JSON.stringify({ label: "rtvi-ai", type: "disconnect-bot", id: rid(), data: {} }));
    } catch {
      // the channel closed under us; closing the call and the POST still tell the desk
    }
    this.teardown(tone);
    await this.request("POST", "/end", {});
  }

  setMuted(muted: boolean) {
    this.mic?.getAudioTracks().forEach((t) => {
      t.enabled = !muted;
    });
  }

  setRoute(route: Route) {
    if (!this.pc) return;
    void (InCallManager.chooseAudioRoute(ROUTE_NAMES[route]) as Promise<unknown>).catch(() => {});
  }

  /** InCallManager reports the routes it can see and the one in use. */
  private onRoute(e: AudioDeviceEvent) {
    let names: unknown = [];
    try {
      names = e.availableAudioDeviceList ? JSON.parse(e.availableAudioDeviceList) : [];
    } catch {
      names = [];
    }
    const available = (Array.isArray(names) ? names : []).map((n) => ROUTE_OF[String(n)]).filter((r): r is Route => !!r);
    const route = ROUTE_OF[e.selectedAudioDevice ?? ""] ?? this.state.audio.route;
    this.set({ audio: { route, available: available.length ? available : [route] } });
    // Settled once the changes stop: a headset reports itself in two or three steps over
    // a second or so (seen, then its call audio up), and the list only names it at the end.
    if (this.routeSettled) {
      if (this.routeQuiet) clearTimeout(this.routeQuiet);
      this.routeQuiet = setTimeout(() => {
        this.routeSettled?.();
        this.routeSettled = null;
        this.routeQuiet = null;
      }, ROUTE_QUIET_MS);
    }
  }

  /** The notification's second line. */
  private callBody(): string {
    const conv = this.state.status?.conversation;
    const line = conv && conv !== "ghost" ? lineName(conv) : "Ghost";
    return this.state.status?.phase === "hold" ? `Checked out from ${line}. Say “Ghost” to bring it back.` : `In a session with ${line}`;
  }

  /**
   * Ghost is hanging up. Once its goodbye has played (speaking, then listening again), the
   * phone closes the call itself, with a hang-up tone, instead of waiting for the desk to
   * wind down; if the goodbye never shows, after a few seconds anyway.
   */
  private hangingUp(status: Status) {
    if (status.phase === "speaking") this.goodbyeHeard = true;
    const done = this.goodbyeHeard && status.phase === "listening";
    if (!this.hangUpTimer) this.hangUpTimer = setTimeout(() => void this.end(true).catch(() => {}), HANG_UP_MS);
    if (done && !this.tailTimer) this.tailTimer = setTimeout(() => void this.end(true).catch(() => {}), HANG_UP_TAIL_MS);
  }

  /** The call dropped and didn't come back. The desk ends its side when the peer goes. */
  private endedUnderUs() {
    if (!this.pc) return;
    this.teardown();
    // Tell the desk too, so it doesn't wait out its ping timeout before Start works again.
    void this.request("POST", "/end", {}).catch(() => {});
    void this.tick();
  }

  private teardown(tone = false) {
    if (this.ping) clearInterval(this.ping);
    this.ping = null;
    if (this.hangUpTimer) clearTimeout(this.hangUpTimer);
    this.hangUpTimer = null;
    if (this.tailTimer) clearTimeout(this.tailTimer);
    this.tailTimer = null;
    this.goodbyeHeard = false;
    const { pc, dc, mic } = this;
    this.pc = null;
    this.dc = null;
    this.mic = null;
    const via = this.state.via ?? "Wi-Fi";
    const seconds = this.callStartedAt ? (Date.now() - this.callStartedAt) / 1000 : 0;
    this.callStartedAt = 0;
    this.routeSub?.remove();
    this.routeSub = null;
    if (pc) {
      stopCall();
      this.set({ audio: DEFAULT_AUDIO });
    }
    try {
      dc?.close();
    } catch {
      // already closed
    }
    if (pc) {
      // The call's numbers (ADR 0018) are read before the close, which empties them; the
      // close itself waits only on that read, a few milliseconds.
      void callStats(pc, via, seconds)
        .then((stats) => {
          if (stats) {
            console.log(`desk: call over ${stats.via}, ${Math.round(stats.seconds)} s, loss ${stats.lossPct ?? "?"}%, jitter ${stats.jitterMs ?? "?"} ms, rtt ${stats.rttMs ?? "?"} ms`);
            this.set({ lastCall: stats });
          }
        })
        .finally(() => {
          try {
            pc.close();
          } catch {
            // already closed
          }
        });
    }
    mic?.getTracks().forEach((t) => t.stop());
    if (pc) {
      try {
        // A hang-up gets the phone's own short end-of-call tone before the audio session closes.
        InCallManager.stop(tone ? { busytone: "_DEFAULT_" } : undefined);
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

type Hello = { workstation: string; status: Status; addresses?: string[] };

/**
 * Loss, jitter and round trip from the phone's WebRTC stats. Loss is on Ghost's voice coming
 * in (what the operator hears); the round trip is the selected candidate pair's.
 */
async function callStats(pc: RTCPeerConnection, via: Via, seconds: number): Promise<CallStats | null> {
  try {
    const report = (await pc.getStats()) as Map<string, Record<string, unknown>> | { forEach: (cb: (v: Record<string, unknown>) => void) => void };
    let lost = 0;
    let received = 0;
    let jitter: number | null = null;
    let rtt: number | null = null;
    let seen = 0;
    const each = (s: Record<string, unknown>) => {
      seen += 1;
      if (s.type === "inbound-rtp" && s.kind === "audio") {
        lost += Number(s.packetsLost ?? 0);
        received += Number(s.packetsReceived ?? 0);
        if (typeof s.jitter === "number") jitter = s.jitter * 1000;
      }
      if (s.type === "candidate-pair" && (s.nominated === true || s.state === "succeeded") && typeof s.currentRoundTripTime === "number") rtt = s.currentRoundTripTime * 1000;
    };
    report.forEach(each);
    if (!seen) return null;
    const total = lost + received;
    return { via, endedAt: new Date().toISOString(), seconds, lossPct: total > 0 ? (100 * lost) / total : null, jitterMs: jitter, rttMs: rtt };
  } catch {
    return null;
  }
}

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
