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
    const url = new URL(String(input instanceof Request ? input.url : input), "http://ghost");
    const method = (init?.method ?? "GET").toUpperCase();
    const body = init?.body == null ? null : String(init.body);
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

/** Enable and start the per-user ghostd service (Tauri only). Resolves to systemctl's output. */
export async function startDaemon(): Promise<string> {
  if (!inTauri) throw new Error("Run `systemctl --user enable --now ghostd` in a terminal.");
  const { invoke } = await import("@tauri-apps/api/core");
  return invoke<string>("start_daemon");
}
