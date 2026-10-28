/**
 * Pick the transport for the environment.
 *
 * - Inside Tauri: HTTP goes through the `api_request` command and events arrive as
 *   `ghost://event` Tauri events; the Rust side owns the Unix socket.
 * - In a browser (Vite dev): plain fetch and WebSocket against the /api proxy.
 */
import { httpTransport, type Transport } from "@ghost/client-ts";

declare global {
  interface Window {
    __TAURI_INTERNALS__?: unknown;
  }
}

export const inTauri = typeof window !== "undefined" && !!window.__TAURI_INTERNALS__;

export async function makeTransport(): Promise<Transport> {
  if (!inTauri) return httpTransport(`${window.location.origin}/api`);
  const { invoke } = await import("@tauri-apps/api/core");
  const { listen } = await import("@tauri-apps/api/event");

  const tauriFetch: typeof fetch = async (input, init) => {
    // openapi-fetch hands over a Request object; its method and body live on it, not on init.
    const req = input instanceof Request ? input : null;
    const url = new URL(String(req ? req.url : input), "http://ghost");
    const method = (init?.method ?? req?.method ?? "GET").toUpperCase();
    let body: string | null = init?.body == null ? null : String(init.body);
    if (body == null && req && method !== "GET" && method !== "HEAD") {
      const t = await req.text();
      body = t === "" ? null : t;
    }
    const r = await invoke<{ status: number; body: string }>("api_request", {
      method,
      path: url.pathname + url.search,
      body,
    });
    return new Response(r.body, {
      status: r.status,
      headers: { "content-type": "application/json" },
    });
  };

  return {
    baseUrl: "http://ghost",
    fetch: tauriFetch,
    events: {
      subscribe(onEvent, onClose) {
        let active = true;
        const unlistenP = listen<string>("ghost://event", (e) => {
          if (active) onEvent(JSON.parse(e.payload));
        });
        const unlistenClosed = listen("ghost://closed", () => {
          if (active) onClose?.();
        });
        void invoke("events_start");
        return () => {
          active = false;
          void unlistenP.then((u) => u());
          void unlistenClosed.then((u) => u());
        };
      },
    },
  };
}

/**
 * What the app's own supervisor last said about the daemon it runs (ADR 0038): adopted
 * one already on the socket, waiting for make dev's, started its own, restarting it after
 * an exit, or stopping on Quit. `restarts` counts the starts after the first.
 */
export type DaemonEvent =
  | { state: "adopted" }
  | { state: "waiting" }
  | { state: "started"; pid: number; restarts: number }
  | { state: "restarting"; in_secs: number; restarts: number; exit: string }
  | { state: "failed"; error: string; in_secs: number }
  | { state: "stopping" }
  | { state: "stopped" };

/** The supervisor's last word, for a window that opened after it; null outside Tauri or before it spoke. */
export async function daemonState(): Promise<DaemonEvent | null> {
  if (!inTauri) return null;
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<DaemonEvent | null>("daemon_state");
}

/** Every change of the daemon's state from now on; returns the unsubscribe. No-op outside Tauri. */
export async function onDaemon(cb: (e: DaemonEvent) => void): Promise<() => void> {
  if (!inTauri) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  return listen<DaemonEvent>("ghost://daemon", (e) => cb(e.payload));
}

/**
 * Start Ghost now (Tauri only): the app runs the daemon itself and starts it again on its
 * own after an exit; this skips the rest of that delay. Under make dev the daemon is the
 * checkout's to start, and this says so.
 */
export async function startDaemon(): Promise<string> {
  if (!inTauri) throw new Error("Run `systemctl --user enable --now ghostd` in a terminal.");
  const { invoke } = await import("@tauri-apps/api/core");
  await invoke("daemon_start");
  return "Starting Ghost";
}
