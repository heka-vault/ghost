import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

/**
 * The commit this window is built from, so the version notice can tell a `make dev` window
 * from the deployed Ghost of the same version (src/app/version.ts, docs/releasing.md). Read
 * once, when Vite builds or the dev server starts, the way ghostd.build reads a checkout:
 * HEAD, and dirty when tracked files have uncommitted changes. Outside git: no commit.
 */
function windowBuild(): { commit: string | null; dirty: boolean } {
  const cwd = fileURLToPath(new URL(".", import.meta.url));
  const git = (...args: string[]) =>
    execFileSync("git", args, { cwd, encoding: "utf8", stdio: ["ignore", "pipe", "ignore"], timeout: 5000 }).trim();
  try {
    return { commit: git("rev-parse", "HEAD") || null, dirty: git("status", "--porcelain", "--untracked-files=no") !== "" };
  } catch {
    return { commit: null, dirty: false };
  }
}

// In development the page runs in a plain browser and the daemon listens on a Unix socket,
// which a browser cannot reach. Vite proxies /api to the socket so `httpTransport("/api")`
// just works. In the Tauri build the Rust side talks to the socket instead.
const socket =
  process.env.GHOST_API_SOCKET ?? `${process.env.XDG_RUNTIME_DIR ?? "/tmp"}/ghost/api.sock`;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  define: { __GHOST_WINDOW_BUILD__: JSON.stringify(windowBuild()) },
  server: {
    port: 1420,
    strictPort: true,
    proxy: {
      "/api": {
        target: { socketPath: socket, host: "ghost" } as never,
        changeOrigin: false,
        ws: true,
        rewrite: (p) => p.replace(/^\/api/, ""),
        // StrictMode opens and closes the first /events WebSocket in the same tick; the
        // daemon has already sent its Status snapshot, and the proxy's write to the closed
        // browser socket fails with EPIPE. Harmless, once per page load. Vite logs it as
        // "ws proxy socket error" from a listener it attaches before this hook runs, so wrap
        // whatever is attached and let everything except EPIPE through, unchanged. Vite runs
        // configure() before it attaches that listener, hence the nextTick.
        configure: (proxy) => {
          proxy.on("proxyReqWs", (_proxyReq, _req, socket) => {
            process.nextTick(() => {
              const existing = socket.listeners("error") as ((err: Error) => void)[];
              socket.removeAllListeners("error");
              socket.on("error", (err: NodeJS.ErrnoException) => {
                if (err.code === "EPIPE") return;
                for (const listener of existing) listener.call(socket, err);
              });
            });
          });
        },
      },
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "es2022", outDir: "dist", sourcemap: false },
});
