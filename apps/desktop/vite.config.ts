import react from "@vitejs/plugin-react";
import { defineConfig } from "vite";

// In development the page runs in a plain browser and the daemon listens on a Unix socket,
// which a browser cannot reach. Vite proxies /api to the socket so `httpTransport("/api")`
// just works. In the Tauri build the Rust side talks to the socket instead.
const socket =
  process.env.GHOST_API_SOCKET ?? `${process.env.XDG_RUNTIME_DIR ?? "/tmp"}/ghost/api.sock`;

export default defineConfig({
  plugins: [react()],
  clearScreen: false,
  server: {
    port: 1420,
    strictPort: true,
    proxy: {
      "/api": {
        target: { socketPath: socket, host: "ghost" } as never,
        changeOrigin: false,
        ws: true,
        rewrite: (p) => p.replace(/^\/api/, ""),
      },
    },
  },
  envPrefix: ["VITE_", "TAURI_ENV_"],
  build: { target: "es2022", outDir: "dist", sourcemap: false },
});
