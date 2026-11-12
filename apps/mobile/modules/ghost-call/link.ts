/**
 * The live connection to the desk (ADR 0051, docs/phone-protocol.md "The live stream"). It is
 * native (`android/.../link/`): a foreground service holds the signed stream with the app
 * closed, posts the notifications and the ring, and sends every action signed with its `id`.
 * This is the app's window onto it. On the web and in mock builds there is no native side:
 * `available` is false and the calls do nothing.
 */
import { NativeModule, requireOptionalNativeModule } from "expo";

type GhostLinkEvents = { state: (e: { json: string }) => void };

declare class GhostLinkNative extends NativeModule<GhostLinkEvents> {
  configure(deviceId: string, secret: string, addresses: string[], workstation: string): void;
  forget(): void;
  getState(): string;
  setEnabled(on: boolean): void;
  setBootStart(on: boolean): void;
  setInSession(on: boolean): void;
  retry(): void;
  act(kind: string, key: string, arg: string | null): Promise<string | null>;
  answerRing(ringId: string): Promise<boolean>;
  declineRing(ringId: string): void;
  permissions(): string;
  openNotificationSettings(): void;
  openChannelSettings(channel: string): void;
  openFullScreenSettings(): void;
}

const native = requireOptionalNativeModule<GhostLinkNative>("GhostLink");

export const linkAvailable = !!native;

/** Hand the pairing to the connection; it keeps it (encrypted) for when the app is closed. */
export function configureLink(deviceId: string, secretB64: string, addresses: string[], workstation: string) {
  native?.configure(deviceId, secretB64, addresses, workstation);
}

export function forgetLink() {
  native?.forget();
}

/** The connection's state as JSON text (`parseLink` in src/desk/waiting.ts reads it), or null. */
export function linkStateJson(): string | null {
  return native ? native.getState() : null;
}

export function onLinkState(cb: (json: string) => void): () => void {
  const sub = native?.addListener("state", (e) => cb(e.json));
  return () => sub?.remove();
}

export function setLinkEnabled(on: boolean) {
  native?.setEnabled(on);
}

export function setLinkBootStart(on: boolean) {
  native?.setBootStart(on);
}

/** A session on this phone: notifications arrive silently and rings are not shown. */
export function setLinkInSession(on: boolean) {
  native?.setInSession(on);
}

export function retryLink() {
  native?.retry();
}

export type LinkActionKind = "ack" | "snooze" | "answer" | "decline" | "confirm" | "cancel";

/** One signed action; resolves null when the desk took it, or with why it didn't. */
export async function linkAct(kind: LinkActionKind, key: string, arg?: string | null): Promise<string | null> {
  if (!native) return "The phone’s connection to your desk isn’t available in this build.";
  return native.act(kind, key, arg ?? null);
}

/** Tell the desk a ring was answered (once, however often asked); true while it was still live. */
export async function answerRing(ringId: string): Promise<boolean> {
  return native ? native.answerRing(ringId) : false;
}

export function declineRing(ringId: string) {
  native?.declineRing(ringId);
}

export interface LinkPermissions {
  /** Notifications allowed for Ghost, and its waiting or questions channel on. */
  notifications: boolean;
  /** The questions-and-changes channel is on. */
  asks: boolean;
  /** The calls channel is on. */
  calls: boolean;
  /** Android 14+: allowed to show a call over the lock screen. */
  fullScreen: boolean;
  /** Android 13+: notifications are a permission the app asks for. */
  needsRuntimePermission: boolean;
}

export function linkPermissions(): LinkPermissions | null {
  if (!native) return null;
  try {
    return JSON.parse(native.permissions()) as LinkPermissions;
  } catch {
    return null;
  }
}

export function openNotificationSettings() {
  native?.openNotificationSettings();
}

export function openFullScreenSettings() {
  native?.openFullScreenSettings();
}
