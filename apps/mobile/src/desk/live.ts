/**
 * The web export has no WebRTC native module; it always runs on the mock desk. Metro picks
 * `live.native.ts` on Android.
 */
import type { Desk } from "./types";

export interface LiveDeskConfig {
  addresses: string[];
  deviceId: string;
  secret: Uint8Array;
  workstation: string;
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
  async start(_conversation?: string, _item?: string) {}
  async end() {}
  async retry() {}
  async loadAudit() {}
  async acknowledge(_key: string) {}
  async snooze(_key: string, _minutes: number) {}
  async answer(_key: string, _text: string) {}
  async decline(_key: string) {}
  async confirmChange(_key: string, _changeId: string) {}
  async cancelChange(_key: string, _changeId: string) {}
  async answerRing(_ringId: string) {
    return false;
  }
  setMuted() {}
  setRoute() {}
  dispose() {}
}
