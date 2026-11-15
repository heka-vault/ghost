// heyghost.app: static, no server, no analytics, nothing fetched from anyone else.
// The hero's float is the desktop app's own floating view (apps/desktop/src/floating) on the
// site's own small mock of the same scene (apps/desktop/src/design/mock/site.ts), imported from
// the source, so the site shows what the app draws.
import { fileURLToPath } from "node:url";

import react from "@astrojs/react";
import sitemap from "@astrojs/sitemap";
import { defineConfig } from "astro/config";

const desktop = fileURLToPath(new URL("../apps/desktop", import.meta.url));

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
    // No commit in the site's build: it would change the float's file names on every deploy and
    // spend each visitor's cache on code that didn't change. The site shows no build.
    define: { __GHOST_WINDOW_BUILD__: "null" },
    server: { fs: { allow: [desktop, fileURLToPath(new URL("..", import.meta.url))] } },
  },
});
