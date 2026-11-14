// heyghost.app: static, no server, no analytics, nothing fetched from anyone else.
// The hero's float is the desktop app's own floating room (apps/desktop/src/floating) and its
// mock daemon, imported from the source, so the site shows what the app draws.
import { execFileSync } from "node:child_process";
import { fileURLToPath } from "node:url";

import react from "@astrojs/react";
import sitemap from "@astrojs/sitemap";
import { defineConfig } from "astro/config";

const desktop = fileURLToPath(new URL("../apps/desktop", import.meta.url));

function commit() {
  try {
    return execFileSync("git", ["rev-parse", "HEAD"], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim() || null;
  } catch {
    return null;
  }
}

export default defineConfig({
  site: "https://heyghost.app",
  trailingSlash: "ignore",
  output: "static",
  integrations: [react(), sitemap({ filter: (page) => !page.includes("/float") })],
  // Nothing on the site calls home; Astro's own telemetry is a build-time setting (ASTRO_TELEMETRY_DISABLED in CI).
  devToolbar: { enabled: false },
  build: { inlineStylesheets: "auto", format: "directory" },
  vite: {
    // The desktop's sources import React and @ghost/ui from their own folder: one copy of each.
    resolve: { dedupe: ["react", "react-dom", "@ghost/ui", "@ghost/client-ts"] },
    define: { __GHOST_WINDOW_BUILD__: JSON.stringify({ commit: commit(), dirty: false }) },
    server: { fs: { allow: [desktop, fileURLToPath(new URL("..", import.meta.url))] } },
  },
});
