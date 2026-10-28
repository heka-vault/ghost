import { defineConfig } from "vitest/config";

/**
 * Unit tests for the desktop's plain modules, in Node. Its own config so vite.config.ts's
 * `define` (the window's commit, read from git) and the React plugin stay out of the tests:
 * a test sets `__GHOST_WINDOW_BUILD__` itself.
 */
export default defineConfig({
  test: {
    include: ["test/**/*.test.ts"],
    environment: "node",
  },
});
