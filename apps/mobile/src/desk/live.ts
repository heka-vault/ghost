/**
 * The web export has no WebRTC native module; it always runs on the mock desk. Metro picks
 * `live.native.ts` on Android.
 */
import type { Desk } from "./types";

export interface LiveDeskConfig {
  address: string;
  deviceId: string;
  secret: Uint8Array;
}

export class LiveDesk implements Desk {
  constructor(_cfg: LiveDeskConfig) {
    throw new Error("The live desk runs only on the phone.");
  }
  getState(): never {
    throw new Error("unreachable");
  }
  subscribe(): never {
    throw new Error("unreachable");
  }
  async start() {}
  async end() {}
  async retry() {}
  async loadAudit() {}
  setMuted() {}
  dispose() {}
}
